use error_chain::ChainedError;
use ffmpeg;
use std::sync::{Mutex, RwLock};

use errors::*;

lazy_static! {
    static ref VIDEO_ENCODER: Mutex<Option<ffmpeg::codec::Video>> = Mutex::new(None);
}

/// An encoder used to encode a video to a file.
///
/// Call `Encoder::start()` to start the encoding, then encode some frames with `Encoder::encode()`.
/// The encoder will flush and save the output file automatically upon being dropped.
struct Encoder {
    converter: ffmpeg::software::scaling::Context,
    context: ffmpeg::format::context::Output,
    encoder: ffmpeg::codec::encoder::Video,
    output_frame: ffmpeg::util::frame::Video,
    packet: ffmpeg::Packet,

    time_base: ffmpeg::Rational,
    stream_time_base: ffmpeg::Rational,

    pts: i64,
}

impl Encoder {
    fn start(filename: &str,
             codec: ffmpeg::codec::Video,
             (width, height): (u32, u32),
             time_base: ffmpeg::Rational) -> Result<Self> {
        let mut context = ffmpeg::format::output(&filename)
            .chain_err(|| "could not create the output context")?;

        let encoder = {
            let mut stream = context.add_stream(codec)
                .chain_err(|| "could not add the video stream")?;

            let mut encoder = stream.codec().encoder().video()
                .chain_err(|| "could not retrieve the video encoder")?;

            encoder.set_width(width);
            encoder.set_height(height);
            encoder.set_time_base(time_base);

            if let Some(mut formats) = codec.formats() {
                encoder.set_format(formats.next().unwrap());
            } else {
                encoder.set_format(ffmpeg::format::Pixel::YUV420P);
            }

            let encoder = encoder.open_as(codec)
                .chain_err(|| "could not open the video encoder")?;
            stream.set_parameters(&encoder);

            stream.set_time_base(time_base);

            encoder
        };

        context.write_header()
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

               time_base,
               stream_time_base,

               pts: 0,
           })
    }

    fn encode(&mut self, frame: &ffmpeg::frame::Video) -> Result<()> {
        self.converter.run(frame, &mut self.output_frame)
            .chain_err(|| "could not convert the frame to the correct format")?;

        self.output_frame.set_pts(Some(self.pts));
        self.pts += 1;

        if self.encoder.encode(&self.output_frame, &mut self.packet)
            .chain_err(|| "could not encode the frame")? {
            self.packet.rescale_ts(self.time_base, self.stream_time_base);

            self.packet.write_interleaved(&mut self.context)
                .chain_err(|| "could not write the packet")?;
        }

        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        while self.encoder.flush(&mut self.packet)
            .chain_err(|| "could not get the packet")? {
            self.packet.rescale_ts(self.time_base, self.stream_time_base);

            self.packet.write_interleaved(&mut self.context)
                .chain_err(|| "could not write the packet")?;
        }

        Ok(())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        #![allow(unused_must_use)]
        self.flush();
        self.context.write_trailer();
    }
}

/// Initialize the encoding stuff. Should be called once.
pub fn initialize() -> Result<()> {
    ffmpeg::init().chain_err(|| "error initializing ffmpeg")?;

    Ok(())
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

command!(cap_test_video_output, |engine| {
    if VIDEO_ENCODER.lock().unwrap().is_none() {
        engine.con_print("Please set the video encoder with cap_set_video_encoder.\n");
        return;
    }

    if let Err(ref e) = test_video_output().chain_err(|| "error in test_video_output()") {
        engine.con_print(&format!("{}", e.display()));
        return;
    }

    engine.con_print("Done!\n");
});

fn test_video_output() -> Result<()> {
    let codec = VIDEO_ENCODER.lock().unwrap();

    ensure!(codec.is_some(), "video encoder was not set");

    let codec = codec.unwrap();

    let mut encoder = Encoder::start("/home/yalter/test.mkv",
                                     codec,
                                     (1280, 720),
                                     (1, 60).into())
        .chain_err(|| "could not create the encoder")?;

    let mut frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGB24, 1280, 720);

    {
        let mut data = frame.plane_mut::<(u8, u8, u8)>(0);
        for pixel in data.iter_mut() {
            *pixel = (0, 255, 0);
        }
    }

    for f in 0..240 {
        {
            let mut data = frame.plane_mut::<(u8, u8, u8)>(0);
            for y in 0..720 {
                data[y * 1280 + f * 2] = (255, 0, 0);
                data[y * 1280 + f * 2 + 1] = (255, 0, 0);
            }
        }

        encoder.encode(&frame)
            .chain_err(|| "could not encode a frame")?;
    }

    Ok(())
}

pub fn get_buffer((width, height): (u32, u32)) -> &'static mut [(u8, u8, u8)] {
    // TODO
    use std::mem;

    lazy_static! {
        static ref FRAME: RwLock<ffmpeg::frame::Video> = RwLock::new(
            ffmpeg::frame::Video::empty()
        );
    }

    let mut frame = FRAME.write().unwrap();

    if frame.width() != width || frame.height() != height {
        println!("Changing resolution from {:?} to {:?}.",
                 (frame.width(), frame.height()),
                 (width, height));

        *frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGB24, width, height);
    }

    unsafe { mem::transmute(frame.plane_mut::<(u8, u8, u8)>(0)) }
}
