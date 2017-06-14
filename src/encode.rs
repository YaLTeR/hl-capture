use error_chain::ChainedError;
use ffmpeg;
use std::{cmp, ptr};
use std::sync::{Mutex, ONCE_INIT, Once};

// use capture;
use errors::*;

lazy_static! {
    static ref VIDEO_ENCODER: Mutex<Option<ffmpeg::codec::Video>> = Mutex::new(None);
    static ref AUDIO_ENCODER: Mutex<Option<ffmpeg::codec::Audio>> = Mutex::new(None);
}

/// An encoder used to encode video and audio to a file.
///
/// Call `Encoder::start()` to start the encoding, then encode some frames with `Encoder::encode()`.
/// The encoder will flush and save the output file automatically upon being dropped.
pub struct Encoder {
    converter: ffmpeg::software::scaling::Context,
    resampler: ffmpeg::software::resampling::Context,
    context: ffmpeg::format::context::Output,
    video_encoder: ffmpeg::codec::encoder::Video,
    audio_encoder: ffmpeg::codec::encoder::Audio,
    video_stream_index: usize,
    audio_stream_index: usize,
    video_output_frame: ffmpeg::util::frame::Video,
    audio_output_frame: ffmpeg::util::frame::Audio,
    audio_input_frame: ffmpeg::util::frame::Audio,
    packet: ffmpeg::Packet,
    finished: bool,

    time_base: ffmpeg::Rational,
    video_stream_time_base: ffmpeg::Rational,
    audio_stream_time_base: ffmpeg::Rational,

    video_pts: i64,
    audio_pts: i64,

    /// Difference, in video frames, between how much time passed in-game and how much video we
    /// output.
    remainder: f64,

    /// Current position, in samples, in the audio frame.
    audio_position: usize,
}

unsafe impl Send for Encoder {}

/// Parameters for encoding and muxing.
pub struct EncoderParameters {
    pub audio_bitrate: usize,
    pub video_bitrate: usize,
    pub crf: String,
    pub filename: String,
    pub muxer_settings: String,
    pub preset: String,
    pub time_base: ffmpeg::Rational,
    pub audio_encoder_settings: String,
    pub video_encoder_settings: String,
    pub vpx_cpu_usage: String,
    pub vpx_threads: String,
}

impl Encoder {
    pub fn start(parameters: &EncoderParameters, (width, height): (u32, u32)) -> Result<Self> {
        let video_codec = VIDEO_ENCODER.lock().unwrap();
        ensure!(video_codec.is_some(), "video encoder was not set");
        let video_codec = video_codec.unwrap();
        let audio_codec = AUDIO_ENCODER.lock().unwrap();
        ensure!(audio_codec.is_some(), "audio encoder was not set");
        let audio_codec = audio_codec.unwrap();

        let mut context = ffmpeg::format::output(&parameters.filename)
            .chain_err(|| "could not create the output context")?;
        let global = context.format().flags().contains(ffmpeg::format::flag::GLOBAL_HEADER);

        let (video_encoder, video_stream_index) = {
            let mut stream =
                context.add_stream(video_codec)
                       .chain_err(|| "could not add the video stream")?;

            let mut encoder =
                stream.codec()
                      .encoder()
                      .video()
                      .chain_err(|| "could not retrieve the video encoder")?;

            if global {
                encoder.set_flags(ffmpeg::codec::flag::GLOBAL_HEADER);
            }

            encoder.set_width(width);
            encoder.set_height(height);
            encoder.set_time_base(parameters.time_base);
            encoder.set_bit_rate(parameters.video_bitrate);

            if let Some(mut formats) = video_codec.formats() {
                encoder.set_format(formats.next().unwrap());
            } else {
                encoder.set_format(ffmpeg::format::Pixel::YUV420P);
            }

            if encoder.format() == ffmpeg::format::Pixel::YUV420P {
                // Write the color space and range into the output file so everything knows how to
                // display it.
                unsafe {
                    (*encoder.as_mut_ptr()).colorspace = ffmpeg::color::Space::BT470BG.into();
                    (*encoder.as_mut_ptr()).color_range = ffmpeg::color::Range::MPEG.into();
                }
            }

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
                          .chain([("crf", parameters.crf.as_str()),
                                  ("preset", parameters.preset.as_str()),
                                  ("cpu-usage", parameters.vpx_cpu_usage.as_str()),
                                  ("threads", parameters.vpx_threads.as_str())]
                                     .iter()
                                     .map(|x| *x)) // By-value iterators?
                          .collect();

            let encoder = encoder.open_as_with(video_codec, encoder_settings)
                                 .chain_err(|| "could not open the video encoder",)?;
            stream.set_parameters(&encoder);

            stream.set_time_base(parameters.time_base);
            unsafe {
                (*stream.as_mut_ptr()).avg_frame_rate = parameters.time_base.invert().into()
            };

            (encoder, stream.index())
        };

        let (audio_encoder, audio_stream_index) = {
            let mut stream =
                context.add_stream(audio_codec)
                       .chain_err(|| "could not add the audio stream")?;

            let mut encoder =
                stream.codec()
                      .encoder()
                      .audio()
                      .chain_err(|| "could not retrieve the audio encoder")?;

            if global {
                encoder.set_flags(ffmpeg::codec::flag::GLOBAL_HEADER);
            }

            encoder.set_bit_rate(parameters.audio_bitrate);

            let rate = if let Some(mut rates) = audio_codec.rates() {
                let mut best_rate = rates.next().unwrap();

                for r in rates {
                    if (r - 22050).abs() < (best_rate - 22050).abs() {
                        best_rate = r;
                    }
                }

                best_rate
            } else {
                22050
            };

            encoder.set_rate(rate);
            encoder.set_time_base((1, rate));

            if let Some(mut formats) = audio_codec.formats() {
                encoder.set_format(formats.next().unwrap());
            } else {
                encoder.set_format(ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed));
            }

            let channel_layout = audio_codec.channel_layouts().map(|cls| cls.best(2)).unwrap_or(ffmpeg::channel_layout::STEREO);
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
                                 .chain_err(|| "could not open the audio encoder",)?;
            stream.set_parameters(&encoder);

            stream.set_time_base((1, rate));

            (encoder, stream.index())
        };

        let muxer_settings = parameters.muxer_settings
            .split_whitespace()
            .filter_map(|s| {
                let mut split = s.splitn(2, '=');

                if let (Some(key), Some(value)) = (split.next(), split.next()) {
                    return Some((key, value));
                }

                None
            }).collect();

        context.write_header_with(muxer_settings)
               .chain_err(|| "could not write the header")?;

        let video_stream_time_base = context.stream(video_stream_index).unwrap().time_base();
        let audio_stream_time_base = context.stream(audio_stream_index).unwrap().time_base();

        let converter = ffmpeg::software::converter((width, height),
                                                    ffmpeg::format::Pixel::RGB24,
                                                    video_encoder.format())
            .chain_err(|| "could not get the color conversion context")?;

        let video_output_frame = ffmpeg::frame::Video::new(video_encoder.format(), width, height);

        let mut audio_frame_size = audio_encoder.frame_size() as usize;
        if audio_frame_size == 0 {
            audio_frame_size = 1024;
        }

        let mut audio_output_frame = ffmpeg::frame::Audio::new(audio_encoder.format(), audio_frame_size, audio_encoder.channel_layout());
        audio_output_frame.set_rate(audio_encoder.rate());

        let mut audio_input_frame = ffmpeg::frame::Audio::new(ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed), audio_frame_size, ffmpeg::channel_layout::STEREO);
        audio_input_frame.set_rate(22050);

        let resampler = ffmpeg::software::resampling::Context::get(audio_input_frame.format(), audio_input_frame.channel_layout(), audio_input_frame.rate(), audio_output_frame.format(), audio_output_frame.channel_layout(), audio_output_frame.rate())
            .chain_err(|| "could not get the resampling context")?;

        println!("audio frame size: {}", audio_frame_size);

        let packet = ffmpeg::Packet::empty();

        Ok(Self {
               converter,
               resampler,
               context,
               video_encoder,
               audio_encoder,
               video_output_frame,
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

               remainder: 0f64,
               audio_position: 0,
           })
    }

    fn push_frame(&mut self) -> Result<()> {
        self.video_output_frame.set_pts(Some(self.video_pts));
        self.video_pts += 1;

        if self.video_encoder
               .encode(&self.video_output_frame, &mut self.packet)
               .chain_err(|| "could not encode the video frame")? {
            self.packet
                .rescale_ts(self.time_base, self.video_stream_time_base);
            self.packet.set_stream(self.video_stream_index);

            self.packet
                .write_interleaved(&mut self.context)
                .chain_err(|| "could not write the video packet")?;
        }

        Ok(())
    }

    fn push_audio_frame(&mut self) -> Result<()> {
        self.audio_output_frame.set_pts(Some(self.audio_pts));
        self.audio_pts += self.audio_output_frame.samples() as i64;

        if self.audio_encoder
               .encode(&self.audio_output_frame, &mut self.packet)
               .chain_err(|| "could not encode the audio frame")? {
            self.packet
                .rescale_ts((1, self.audio_output_frame.rate() as i32), self.audio_stream_time_base);
            self.packet.set_stream(self.audio_stream_index);

            self.packet
                .write_interleaved(&mut self.context)
                .chain_err(|| "could not write the audio packet")?;
        }

        Ok(())
    }

    pub fn take(&mut self, frame: &ffmpeg::frame::Video, frametime: f64) -> Result<()> {
        // capture::CAPTURE_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("color conversion"));
        self.converter
            .run(frame, &mut self.video_output_frame)
            .chain_err(|| "could not convert the frame to the correct format")?;

        let time_base: f64 = self.time_base.into();
        self.remainder += frametime / time_base;

        // capture::CAPTURE_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("push_frame()"));
        loop {
            // Push this frame as long as it takes up the most of the video frame.
            // TODO: move this logic somewhere to skip glReadPixels and other stuff
            // altogether if we're gonna drop this frame anyway.
            if self.remainder <= 0.5f64 {
                break;
            }

            self.push_frame()?;
            self.remainder -= 1f64;
        }

        Ok(())
    }

    /// Encodes 16-bit signed interleaved 2-channel stereo sound.
    pub fn take_audio(&mut self, samples: &Vec<(i16, i16)>) -> Result<()> {
        let mut samples_pos = 0;
        while samples_pos < samples.len() {
            let available_samples = samples.len() - samples_pos;
            let available_space = self.audio_input_frame.samples() - self.audio_position;
            let to_move = cmp::min(available_samples, available_space);

            unsafe {
                ptr::copy_nonoverlapping(samples[samples_pos..samples_pos + to_move].as_ptr() as *const u8,
                                         self.audio_input_frame.data_mut(0)[self.audio_position * 4..(self.audio_position + to_move) * 4].as_mut_ptr(),
                                         to_move * 4);
            }

            samples_pos += to_move;
            self.audio_position += to_move;

            if self.audio_position == self.audio_input_frame.samples() {
                self.resampler.run(&self.audio_input_frame, &mut self.audio_output_frame)
                    .chain_err(|| "could not resample the sound")?;
                self.push_audio_frame()?;

                while let Some(_) = self.resampler.delay() {
                    self.resampler.flush(&mut self.audio_output_frame)
                        .chain_err(|| "could not resample the sound")?;
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
                  .chain_err(|| "could not get the packet")? {
            self.packet
                .rescale_ts(self.time_base, self.video_stream_time_base);
            self.packet.set_stream(self.video_stream_index);

            self.packet
                .write_interleaved(&mut self.context)
                .chain_err(|| "could not write the packet")?;
        }

        // Fill the remaining audio buffer with silence and encode it.
        if self.audio_position > 0 {
            let available_space = self.audio_input_frame.samples() - self.audio_position;
            unsafe {
                ptr::write_bytes(self.audio_input_frame.data_mut(0)[self.audio_position * 4..(self.audio_position + available_space) * 4].as_mut_ptr(), 0, available_space * 4);
            }

            self.resampler.run(&self.audio_input_frame, &mut self.audio_output_frame)
                .chain_err(|| "could not resample the sound")?;
            self.push_audio_frame()?;

            while let Some(_) = self.resampler.delay() {
                self.resampler.flush(&mut self.audio_output_frame)
                    .chain_err(|| "could not resample the sound")?;
                self.push_audio_frame()?;
            }

            self.audio_position = 0;
        }

        while self.audio_encoder
                  .flush(&mut self.packet)
                  .chain_err(|| "could not get the packet")? {
            self.packet
                .rescale_ts((1, self.audio_output_frame.rate() as i32), self.audio_stream_time_base);
            self.packet.set_stream(self.audio_stream_index);

            self.packet
                .write_interleaved(&mut self.context)
                .chain_err(|| "could not write the packet")?;
        }

        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        // This should be at the beginning because we want to be able to drop the Encoder even if
        // stuff here fails.
        self.finished = true;

        self.flush().chain_err(|| "unable to flush the encoder")?;
        self.context
            .write_trailer()
            .chain_err(|| "could not write the trailer")?;

        Ok(())
    }

    pub fn width(&self) -> u32 {
        self.video_encoder.width()
    }

    pub fn height(&self) -> u32 {
        self.video_encoder.height()
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if !self.finished {
            panic!("dropped an Encoder that was not properly closed (see Encoder::finish())");
        }
    }
}

/// Initialize the encoding stuff.
pub fn initialize() {
    static INIT: Once = ONCE_INIT;

    INIT.call_once(|| {
        if let Err(ref e) = ffmpeg::init().chain_err(|| "error initializing ffmpeg") {
            panic!("{}", e.display());
        }

        *VIDEO_ENCODER.lock().unwrap() = ffmpeg::encoder::find_by_name("libx264")
            .and_then(|e| e.video().ok());
        *AUDIO_ENCODER.lock().unwrap() = ffmpeg::encoder::find_by_name("aac")
            .and_then(|e| e.audio().ok());
    });
}

command!(cap_set_video_encoder, |engine| {
    let mut args = engine.args();

    if args.len() != 2 {
        let mut buf = String::new();

        buf.push_str("Usage:\n");
        buf.push_str("    cap_set_video_encoder <encoder>\n");
        buf.push_str("     - Set the video encoder by name.\n");
        buf.push_str("Example:\n");
        buf.push_str("    cap_set_video_encoder libx264\n");

        engine.con_print(&buf);
        return;
    }

    let encoder_name = args.nth(1).unwrap();

    if let Some(encoder) = ffmpeg::encoder::find_by_name(&encoder_name) {
        if let Ok(video) = encoder.video() {
            let mut buf = String::new();

            buf.push_str(&format!("Selected encoder: {}\n", encoder_name));
            buf.push_str(&format!("Description: {}\n", encoder.description()));
            buf.push_str("Pixel formats: ");

            if let Some(formats) = video.formats() {
                buf.push_str(&format!("{:?}\n", formats.collect::<Vec<_>>()));
            } else {
                buf.push_str("any\n");
            }

            engine.con_print(&buf);

            *VIDEO_ENCODER.lock().unwrap() = Some(video);
        } else {
            engine.con_print(&format!("Invalid encoder type '{}'\n", encoder_name));
        }
    } else {
        engine.con_print(&format!("Unknown encoder '{}'\n", encoder_name));
    }
});

command!(cap_set_audio_encoder, |engine| {
    let mut args = engine.args();

    if args.len() != 2 {
        let mut buf = String::new();

        buf.push_str("Usage:\n");
        buf.push_str("    cap_set_audio_encoder <encoder>\n");
        buf.push_str("     - Set the audio encoder by name.\n");
        buf.push_str("Example:\n");
        buf.push_str("    cap_set_audio_encoder libmp3lame\n");

        engine.con_print(&buf);
        return;
    }

    let encoder_name = args.nth(1).unwrap();

    if let Some(encoder) = ffmpeg::encoder::find_by_name(&encoder_name) {
        if let Ok(audio) = encoder.audio() {
            let mut buf = String::new();

            buf.push_str(&format!("Selected encoder: {}\n", encoder_name));
            buf.push_str(&format!("Description: {}\n", encoder.description()));
            buf.push_str("Sample formats: ");

            if let Some(formats) = audio.formats() {
                buf.push_str(&format!("{:?}\n", formats.collect::<Vec<_>>()));
            } else {
                buf.push_str("any\n");
            }

            buf.push_str("Sample rates: ");

            if let Some(rates) = audio.rates() {
                buf.push_str(&format!("{:?}\n", rates.collect::<Vec<_>>()));
            } else {
                buf.push_str("any\n");
            }

            engine.con_print(&buf);

            *AUDIO_ENCODER.lock().unwrap() = Some(audio);
        } else {
            engine.con_print(&format!("Invalid encoder type '{}'\n", encoder_name));
        }
    } else {
        engine.con_print(&format!("Unknown encoder '{}'\n", encoder_name));
    }
});

// command!(cap_test_video_output, |engine| {
//     if VIDEO_ENCODER.lock().unwrap().is_none() {
//         engine.con_print("Please set the video encoder with cap_set_video_encoder.\n");
//         return;
//     }
//
//     if let Err(ref e) = test_video_output().chain_err(|| "error in test_video_output()") {
//         engine.con_print(&format!("{}", e.display()));
//         return;
//     }
//
//     engine.con_print("Done!\n");
// });
//
// fn test_video_output() -> Result<()> {
//     let mut encoder = Encoder::start("/home/yalter/test.mkv",
//                                      (1680, 1050),
//                                      (1, 60).into())
//         .chain_err(|| "could not create the encoder")?;
//
//     let mut frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGB24, 1680, 1050);
//
//     {
//         let mut data = frame.plane_mut::<(u8, u8, u8)>(0);
//         for pixel in data.iter_mut() {
//             *pixel = (0, 255, 0);
//         }
//     }
//
//     let linesize = unsafe {
//         (*frame.as_ptr()).linesize[0] / 3
//     };
//
//     for f in 0..240 {
//         {
//             let mut data = frame.plane_mut::<(u8, u8, u8)>(0);
//             for y in 0..1050 {
//                 data[y * linesize as usize + f * 2] = (255, 0, 0);
//                 data[y * linesize as usize + f * 2 + 1] = (255, 0, 0);
//             }
//         }
//
//         encoder.encode(&frame)
//             .chain_err(|| "could not encode a frame")?;
//     }
//
//     Ok(())
// }
