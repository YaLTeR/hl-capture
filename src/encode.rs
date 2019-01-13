use failure::{bail, ensure, Error, ResultExt};
use ffmpeg::channel_layout::{self, ChannelLayout};
use ffmpeg::codec::{self, encoder};
use ffmpeg::format::{self, context};
use ffmpeg::software::{self, resampling, scaling};
use ffmpeg::util::frame;
use ffmpeg::{self, color, Packet, Rational};
use lazy_static::lazy_static;
use std::cmp;
use std::result;
use std::sync::{Mutex, Once, ONCE_INIT};

use crate::utils::format_error;

type Result<T> = result::Result<T, Error>;

lazy_static! {
    static ref VIDEO_ENCODER: Mutex<Option<codec::Video>> = Mutex::new(None);
    static ref AUDIO_ENCODER: Mutex<Option<codec::Audio>> = Mutex::new(None);
}

const HL_SAMPLE_FORMAT: format::Sample = format::Sample::I16(format::sample::Type::Packed);
const HL_SAMPLE_RATE: i32 = 22050;
const HL_CHANNEL_LAYOUT: ChannelLayout = channel_layout::STEREO;

/// An encoder used to encode video and audio to a file.
///
/// Call `Encoder::start()` to start the encoding, then encode some frames with `Encoder::encode()`.
/// The encoder will flush and save the output file automatically upon being dropped.
pub struct Encoder {
    converter: PixFmtConverter,
    resampler: resampling::Context,
    context: context::Output,
    video_encoder: encoder::Video,
    audio_encoder: encoder::Audio,
    video_stream_index: usize,
    audio_stream_index: usize,
    audio_output_frame: frame::Audio,
    audio_input_frame: frame::Audio,
    packet: Packet,
    finished: bool,

    time_base: Rational,
    video_stream_time_base: Rational,
    audio_stream_time_base: Rational,

    video_pts: i64,
    audio_pts: i64,

    /// Current position, in samples, in the audio frame.
    audio_position: usize,
}

/// Parameters for encoding and muxing.
pub struct EncoderParameters {
    pub audio_bitrate: usize,
    pub video_bitrate: usize,
    pub crf: String,
    pub filename: String,
    pub muxer_settings: String,
    pub pixel_format: format::Pixel,
    pub preset: String,
    pub time_base: Rational,
    pub audio_encoder_settings: String,
    pub video_encoder_settings: String,
    pub vpx_threads: String,
    pub video_resolution: (u32, u32),
}

/// Lazily-initialized pixel format converter.
struct PixFmtConverter {
    inner: Option<PixFmtConverterInner>,
    output_format: format::Pixel,
}

/// Pixel format converter.
struct PixFmtConverterInner {
    context: scaling::Context,
    output_frame: frame::Video,
}

impl Encoder {
    pub fn start(parameters: &EncoderParameters) -> Result<Self> {
        let video_codec = VIDEO_ENCODER.lock().unwrap();
        ensure!(video_codec.is_some(), "video encoder was not set");
        let video_codec = video_codec.unwrap();
        let audio_codec = AUDIO_ENCODER.lock().unwrap();
        ensure!(audio_codec.is_some(), "audio encoder was not set");
        let audio_codec = audio_codec.unwrap();

        let mut context =
            format::output(&parameters.filename).context({
                                                    "could not create the output context"
                                                })?;
        let global = context.format()
                            .flags()
                            .contains(format::flag::GLOBAL_HEADER);

        let (video_encoder, video_stream_index) = {
            let mut stream = context.add_stream(video_codec)
                                    .context("could not add the video stream")?;

            let mut encoder = stream.codec()
                                    .encoder()
                                    .video()
                                    .context("could not retrieve the video encoder")?;

            if global {
                encoder.set_flags(codec::flag::GLOBAL_HEADER);
            }

            let (width, height) = parameters.video_resolution;
            encoder.set_width(width);
            encoder.set_height(height);
            encoder.set_time_base(parameters.time_base);
            encoder.set_bit_rate(parameters.video_bitrate);

            if let Some(mut formats) = video_codec.formats() {
                if parameters.pixel_format == format::Pixel::None {
                    encoder.set_format(formats.next().unwrap());
                } else {
                    ensure!(formats.any(|x| x == parameters.pixel_format),
                            "the selected video encoder does not support the chosen pixel format");
                    encoder.set_format(parameters.pixel_format);
                }
            } else {
                if parameters.pixel_format == format::Pixel::None {
                    encoder.set_format(format::Pixel::YUV420P);
                } else {
                    encoder.set_format(parameters.pixel_format);
                }
            }

            if encoder.format() == format::Pixel::YUV420P
               || encoder.format() == format::Pixel::YUV444P
            {
                // Write the color space and range into the output file so everything knows how to
                // display it.
                encoder.set_colorspace(color::Space::BT470BG);
                encoder.set_color_range(color::Range::MPEG);
            }

            let extra_settings = [("crf", &parameters.crf),
                                  ("preset", &parameters.preset),
                                  ("threads", &parameters.vpx_threads)];
            let extra_settings =
                extra_settings.iter().filter_map(|&(name, value)| {
                                         value.split_whitespace().next().map(|v| (name, v))
                                     });

            let encoder_settings =
                parameters.video_encoder_settings
                          .split_whitespace()
                          .filter_map(|s| {
                              let mut split = s.splitn(2, '=');

                              if let (Some(key), Some(value)) = (split.next(), split.next()) {
                                  return Some((key, value));
                              }

                              None
                          })
                          .chain(extra_settings)
                          .collect();

            let encoder = encoder.open_as_with(video_codec, encoder_settings)
                                 .context("could not open the video encoder")?;
            stream.set_parameters(&encoder);

            stream.set_time_base(parameters.time_base);
            stream.set_avg_frame_rate(parameters.time_base.invert());

            (encoder, stream.index())
        };

        let (audio_encoder, audio_stream_index) = {
            let mut stream = context.add_stream(audio_codec)
                                    .context("could not add the audio stream")?;

            let mut encoder = stream.codec()
                                    .encoder()
                                    .audio()
                                    .context("could not retrieve the audio encoder")?;

            if global {
                encoder.set_flags(codec::flag::GLOBAL_HEADER);
            }

            encoder.set_bit_rate(parameters.audio_bitrate);

            let rate = if let Some(mut rates) = audio_codec.rates() {
                let mut best_rate = rates.next().unwrap();

                for r in rates {
                    if (r - HL_SAMPLE_RATE).abs() < (best_rate - HL_SAMPLE_RATE).abs() {
                        best_rate = r;
                    }
                }

                best_rate
            } else {
                HL_SAMPLE_RATE
            };

            encoder.set_rate(rate);
            encoder.set_time_base((1, rate));

            if let Some(mut formats) = audio_codec.formats() {
                encoder.set_format(formats.next().unwrap());
            } else {
                encoder.set_format(HL_SAMPLE_FORMAT);
            }

            let channel_layout = audio_codec.channel_layouts()
                                            .map(|cls| cls.best(HL_CHANNEL_LAYOUT.channels()))
                                            .unwrap_or(channel_layout::STEREO);
            encoder.set_channel_layout(channel_layout);
            encoder.set_channels(channel_layout.channels());

            let encoder_settings =
                parameters.audio_encoder_settings
                          .split_whitespace()
                          .filter_map(|s| {
                              let mut split = s.splitn(2, '=');

                              if let (Some(key), Some(value)) = (split.next(), split.next()) {
                                  return Some((key, value));
                              }

                              None
                          })
                          .collect();

            let encoder = encoder.open_as_with(audio_codec, encoder_settings)
                                 .context("could not open the audio encoder")?;
            stream.set_parameters(&encoder);

            stream.set_time_base((1, rate));

            (encoder, stream.index())
        };

        let muxer_settings =
            parameters.muxer_settings
                      .split_whitespace()
                      .filter_map(|s| {
                          let mut split = s.splitn(2, '=');

                          if let (Some(key), Some(value)) = (split.next(), split.next()) {
                              return Some((key, value));
                          }

                          None
                      })
                      .collect();

        context.write_header_with(muxer_settings)
               .context("could not write the header")?;

        let video_stream_time_base = context.stream(video_stream_index).unwrap().time_base();
        let audio_stream_time_base = context.stream(audio_stream_index).unwrap().time_base();

        let mut audio_frame_size = audio_encoder.frame_size() as usize;
        if audio_frame_size == 0 {
            audio_frame_size = 1024;
        }

        let mut audio_output_frame = frame::Audio::new(audio_encoder.format(),
                                                       audio_frame_size,
                                                       audio_encoder.channel_layout());
        audio_output_frame.set_rate(audio_encoder.rate());

        let mut audio_input_frame =
            frame::Audio::new(HL_SAMPLE_FORMAT, audio_frame_size, HL_CHANNEL_LAYOUT);
        audio_input_frame.set_rate(HL_SAMPLE_RATE as u32);

        let resampler = software::resampler(
            (
                audio_input_frame.format(),
                audio_input_frame.channel_layout(),
                audio_input_frame.rate(),
            ),
            (
                audio_output_frame.format(),
                audio_output_frame.channel_layout(),
                audio_output_frame.rate(),
            ),
        ).context("could not get the resampling context")?;

        let packet = Packet::empty();

        Ok(Self { converter: PixFmtConverter::new(video_encoder.format()),
                  resampler,
                  context,
                  video_encoder,
                  audio_encoder,
                  audio_output_frame,
                  audio_input_frame,
                  video_stream_index,
                  audio_stream_index,
                  packet,
                  finished: false,

                  time_base: parameters.time_base,
                  video_stream_time_base,
                  audio_stream_time_base,

                  video_pts: 0,
                  audio_pts: 0,

                  audio_position: 0 })
    }

    fn push_frame(&mut self, frame: Option<&mut frame::Video>, times: usize) -> Result<()> {
        let frame = frame.or(self.converter.output_frame()).unwrap();

        for _ in 0..times {
            frame.set_pts(Some(self.video_pts));
            self.video_pts += 1;

            if self.video_encoder
                   .encode(frame, &mut self.packet)
                   .context("could not encode the video frame")?
            {
                self.packet
                    .rescale_ts(self.time_base, self.video_stream_time_base);
                self.packet.set_stream(self.video_stream_index);

                self.packet
                    .write_interleaved(&mut self.context)
                    .context("could not write the video packet")?;
            }
        }

        Ok(())
    }

    fn push_audio_frame(&mut self) -> Result<()> {
        self.audio_output_frame.set_pts(Some(self.audio_pts));
        self.audio_pts += self.audio_output_frame.samples() as i64;

        if self.audio_encoder
               .encode(&self.audio_output_frame, &mut self.packet)
               .context("could not encode the audio frame")?
        {
            self.packet
                .rescale_ts((1, self.audio_output_frame.rate() as i32),
                            self.audio_stream_time_base);
            self.packet.set_stream(self.audio_stream_index);

            self.packet
                .write_interleaved(&mut self.context)
                .context("could not write the audio packet")?;
        }

        Ok(())
    }

    /// Takes the given frame the specified number of times.
    pub fn take(&mut self, frame: &mut frame::Video, times: usize) -> Result<()> {
        if frame.height() != self.height() || frame.width() != self.width() {
            panic!("the given frame's size doesn't match the encoder frame size");
        }

        let input = if frame.format() != self.format() {
            self.converter.convert(frame)?;
            None
        } else {
            Some(frame)
        };

        self.push_frame(input, times)?;

        Ok(())
    }

    /// Encodes 16-bit signed interleaved 2-channel stereo sound.
    pub fn take_audio(&mut self, samples: &[(i16, i16)]) -> Result<()> {
        let mut samples_pos = 0;
        while samples_pos < samples.len() {
            let available_samples = samples.len() - samples_pos;
            let available_space = self.audio_input_frame.samples() - self.audio_position;
            let to_move = cmp::min(available_samples, available_space);

            for i in 0..to_move {
                self.audio_input_frame.plane_mut(0)[self.audio_position + i] =
                    samples[samples_pos + i];
            }

            samples_pos += to_move;
            self.audio_position += to_move;

            if self.audio_position == self.audio_input_frame.samples() {
                self.resampler
                    .run(&self.audio_input_frame, &mut self.audio_output_frame)
                    .context("could not resample the sound")?;
                self.push_audio_frame()?;

                while let Some(_) = self.resampler.delay() {
                    self.resampler
                        .flush(&mut self.audio_output_frame)
                        .context("could not resample the sound")?;
                    self.push_audio_frame()?;
                }

                self.audio_position = 0;
            }
        }

        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        while self.video_encoder
                  .flush(&mut self.packet)
                  .context("could not get the packet")?
        {
            self.packet
                .rescale_ts(self.time_base, self.video_stream_time_base);
            self.packet.set_stream(self.video_stream_index);

            self.packet
                .write_interleaved(&mut self.context)
                .context("could not write the packet")?;
        }

        // Fill the remaining audio buffer with silence and encode it.
        if self.audio_position > 0 {
            let available_space = self.audio_input_frame.samples() - self.audio_position;
            for i in 0..available_space {
                self.audio_input_frame.plane_mut(0)[i] = (0i16, 0i16);
            }

            self.resampler
                .run(&self.audio_input_frame, &mut self.audio_output_frame)
                .context("could not resample the sound")?;
            self.push_audio_frame()?;

            while let Some(_) = self.resampler.delay() {
                self.resampler
                    .flush(&mut self.audio_output_frame)
                    .context("could not resample the sound")?;
                self.push_audio_frame()?;
            }

            self.audio_position = 0;
        }

        while self.audio_encoder
                  .flush(&mut self.packet)
                  .context("could not get the packet")?
        {
            self.packet
                .rescale_ts((1, self.audio_output_frame.rate() as i32),
                            self.audio_stream_time_base);
            self.packet.set_stream(self.audio_stream_index);

            self.packet
                .write_interleaved(&mut self.context)
                .context("could not write the packet")?;
        }

        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        // This should be at the beginning because we want to be able to drop the Encoder even if
        // stuff here fails.
        self.finished = true;

        self.flush().context("unable to flush the encoder")?;
        self.context
            .write_trailer()
            .context("could not write the trailer")?;

        Ok(())
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.video_encoder.width()
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.video_encoder.height()
    }

    #[inline]
    pub fn format(&self) -> format::Pixel {
        self.video_encoder.format()
    }
}

impl Drop for Encoder {
    #[inline]
    fn drop(&mut self) {
        if !self.finished {
            panic!("dropped an Encoder that was not properly closed (see Encoder::finish())");
        }
    }
}

impl PixFmtConverter {
    #[inline]
    fn new(output_format: format::Pixel) -> Self {
        Self { inner: None,
               output_format }
    }

    fn convert(&mut self, frame: &frame::Video) -> Result<&mut frame::Video> {
        if self.inner.is_none() || self.inner.as_ref().unwrap().format() != frame.format() {
            self.inner = Some(PixFmtConverterInner::new((frame.width(), frame.height()),
                                                        frame.format(),
                                                        self.output_format)?);
        }

        self.inner.as_mut().unwrap().convert(frame)
    }

    #[inline]
    fn output_frame(&mut self) -> Option<&mut frame::Video> {
        self.inner.as_mut().map(|x| &mut x.output_frame)
    }
}

impl PixFmtConverterInner {
    #[inline]
    fn new((width, height): (u32, u32),
           input: format::Pixel,
           output: format::Pixel)
           -> Result<Self> {
        Ok(Self {
            context: software::converter((width, height), input, output)
                .context("could not initialize the color conversion context")?,
            output_frame: frame::Video::new(output, width, height),
        })
    }

    #[inline]
    fn convert(&mut self, frame: &frame::Video) -> Result<&mut frame::Video> {
        self.context
            .run(frame, &mut self.output_frame)
            .context("could not convert the frame to the correct color format")?;

        Ok(&mut self.output_frame)
    }

    #[inline]
    fn format(&self) -> format::Pixel {
        self.context.input().format
    }
}

/// Initialize the encoding stuff.
pub fn initialize() {
    static INIT: Once = ONCE_INIT;

    INIT.call_once(|| {
            if let Err(e) = ffmpeg::init().context("error initializing ffmpeg") {
                panic!("{}", format_error(&e.into()));
            }

            *VIDEO_ENCODER.lock().unwrap() =
                encoder::find_by_name("libx264").and_then(|e| e.video().ok());
            *AUDIO_ENCODER.lock().unwrap() =
                encoder::find_by_name("aac").and_then(|e| e.audio().ok());
        });
}

/// Outputs information about the selected video encoder.
fn video_encoder_info(buf: &mut String) {
    if let Some(encoder) = *VIDEO_ENCODER.lock().unwrap() {
        let video = encoder.video().unwrap();

        buf.push_str(&format!("Selected encoder: {}\n", encoder.name()));
        buf.push_str(&format!("Description: {}\n", encoder.description()));
        buf.push_str("Supported pixel formats: ");

        if let Some(formats) = video.formats() {
            buf.push_str(&format!("{:?}\n", formats.collect::<Vec<_>>()));
        } else {
            buf.push_str("any\n");
        }
    } else {
        buf.push_str("No video encoder is currently selected.\n");
    }
}

/// Outputs information about the selected audio encoder.
fn audio_encoder_info(buf: &mut String) {
    if let Some(encoder) = *AUDIO_ENCODER.lock().unwrap() {
        let audio = encoder.audio().unwrap();

        buf.push_str(&format!("Selected encoder: {}\n", encoder.name()));
        buf.push_str(&format!("Description: {}\n", encoder.description()));

        buf.push_str("Supported sample rates: ");

        if let Some(rates) = audio.rates() {
            buf.push_str(&format!("{:?}\n", rates.collect::<Vec<_>>()));
        } else {
            buf.push_str("any\n");
        }
    } else {
        buf.push_str("No audio encoder is currently selected.\n");
    }
}

command!(cap_video_encoder, |marker| {
    let engine = marker.engine();
    let mut args = engine.args();

    if args.len() != 2 {
        let mut buf = String::new();

        video_encoder_info(&mut buf);

        buf.push_str("\nUsage:\n");
        buf.push_str("    cap_video_encoder <encoder>\n");
        buf.push_str("     - Set the video encoder by name.\n");
        buf.push_str("    cap_video_encoder\n");
        buf.push_str("     - Show information about the current video encoder.\n");
        buf.push_str("Example:\n");
        buf.push_str("    cap_video_encoder libx264\n");

        engine.con_print(&buf);
        return;
    }

    let encoder_name = args.nth(1).unwrap();

    if let Some(encoder) = encoder::find_by_name(&encoder_name) {
        if let Ok(video) = encoder.video() {
            *VIDEO_ENCODER.lock().unwrap() = Some(video);

            let mut buf = String::new();
            video_encoder_info(&mut buf);
            engine.con_print(&buf);
        } else {
            engine.con_print(&format!("Invalid encoder type '{}'\n", encoder_name));
        }
    } else {
        engine.con_print(&format!("Unknown encoder '{}'\n", encoder_name));
    }
});

command!(cap_audio_encoder, |marker| {
    let engine = marker.engine();
    let mut args = engine.args();

    if args.len() != 2 {
        let mut buf = String::new();

        audio_encoder_info(&mut buf);

        buf.push_str("\nUsage:\n");
        buf.push_str("    cap_audio_encoder <encoder>\n");
        buf.push_str("     - Set the audio encoder by name.\n");
        buf.push_str("    cap_audio_encoder\n");
        buf.push_str("     - Show information about the current audio encoder.\n");
        buf.push_str("Example:\n");
        buf.push_str("    cap_audio_encoder aac\n");

        engine.con_print(&buf);
        return;
    }

    let encoder_name = args.nth(1).unwrap();

    if let Some(encoder) = encoder::find_by_name(&encoder_name) {
        if let Ok(audio) = encoder.audio() {
            *AUDIO_ENCODER.lock().unwrap() = Some(audio);

            let mut buf = String::new();
            audio_encoder_info(&mut buf);
            engine.con_print(&buf);
        } else {
            engine.con_print(&format!("Invalid encoder type '{}'\n", encoder_name));
        }
    } else {
        engine.con_print(&format!("Unknown encoder '{}'\n", encoder_name));
    }
});
