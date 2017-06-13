use error_chain::ChainedError;
use ffmpeg;
use std::sync::{Mutex, ONCE_INIT, Once};

use capture;
use errors::*;

lazy_static! {
    static ref VIDEO_ENCODER: Mutex<Option<ffmpeg::codec::Video>> = Mutex::new(None);
}

/// An encoder used to encode a video to a file.
///
/// Call `Encoder::start()` to start the encoding, then encode some frames with `Encoder::encode()`.
/// The encoder will flush and save the output file automatically upon being dropped.
pub struct Encoder {
    converter: ffmpeg::software::scaling::Context,
    context: ffmpeg::format::context::Output,
    encoder: ffmpeg::codec::encoder::Video,
    output_frame: ffmpeg::util::frame::Video,
    packet: ffmpeg::Packet,
    finished: bool,

    time_base: ffmpeg::Rational,
    stream_time_base: ffmpeg::Rational,

    pts: i64,

    /// Difference, in video frames, between how much time passed in-game and how much video we
    /// output.
    remainder: f64,
}

unsafe impl Send for Encoder {}

/// Parameters for encoding and muxing.
pub struct EncoderParameters {
    pub bitrate: usize,
    pub crf: String,
    pub filename: String,
    pub muxer_settings: String,
    pub preset: String,
    pub time_base: ffmpeg::Rational,
    pub video_encoder_settings: String,
    pub vpx_cpu_usage: String,
    pub vpx_threads: String,
}

impl Encoder {
    pub fn start(parameters: &EncoderParameters, (width, height): (u32, u32)) -> Result<Self> {
        let codec = VIDEO_ENCODER.lock().unwrap();
        ensure!(codec.is_some(), "video encoder was not set");
        let codec = codec.unwrap();

        let mut context = ffmpeg::format::output(&parameters.filename)
            .chain_err(|| "could not create the output context")?;
        let global = context.format().flags().contains(ffmpeg::format::flag::GLOBAL_HEADER);

        let encoder = {
            let mut stream =
                context.add_stream(codec)
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
            encoder.set_bit_rate(parameters.bitrate);

            if let Some(mut formats) = codec.formats() {
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

            let encoder = encoder.open_as_with(codec, encoder_settings)
                                 .chain_err(|| "could not open the video encoder",)?;
            stream.set_parameters(&encoder);

            stream.set_time_base(parameters.time_base);
            unsafe {
                (*stream.as_mut_ptr()).avg_frame_rate = parameters.time_base.invert().into()
            };

            encoder
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

        let stream_time_base = context.stream(0).unwrap().time_base();

        let converter = ffmpeg::software::converter((width, height),
                                                    ffmpeg::format::Pixel::RGB24,
                                                    encoder.format())
            .chain_err(|| "could not get the color conversion context")?;

        let output_frame = ffmpeg::frame::Video::new(encoder.format(), width, height);

        let packet = ffmpeg::Packet::empty();

        Ok(Self {
               converter,
               context,
               encoder,
               output_frame,
               packet,
               finished: false,

               time_base: parameters.time_base,
               stream_time_base,

               pts: 0,
               remainder: 0f64,
           })
    }

    fn push_frame(&mut self) -> Result<()> {
        self.output_frame.set_pts(Some(self.pts));
        self.pts += 1;

        if self.encoder
               .encode(&self.output_frame, &mut self.packet)
               .chain_err(|| "could not encode the frame")? {
            self.packet
                .rescale_ts(self.time_base, self.stream_time_base);

            self.packet
                .write_interleaved(&mut self.context)
                .chain_err(|| "could not write the packet")?;
        }

        Ok(())
    }

    pub fn take(&mut self, frame: &ffmpeg::frame::Video, frametime: f64) -> Result<()> {
        capture::CAPTURE_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("color conversion"));
        self.converter
            .run(frame, &mut self.output_frame)
            .chain_err(|| "could not convert the frame to the correct format")?;

        let time_base: f64 = self.time_base.into();
        self.remainder += frametime / time_base;

        capture::CAPTURE_THREAD_PROFILER.with(|p| p.borrow_mut().as_mut().unwrap().start_section("push_frame()"));
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

    fn flush(&mut self) -> Result<()> {
        while self.encoder
                  .flush(&mut self.packet)
                  .chain_err(|| "could not get the packet")? {
            self.packet
                .rescale_ts(self.time_base, self.stream_time_base);

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
        self.encoder.width()
    }

    pub fn height(&self) -> u32 {
        self.encoder.height()
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
