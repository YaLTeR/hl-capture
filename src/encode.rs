use error_chain::ChainedError;
use ffmpeg;
use std::sync::Mutex;

use errors::*;

lazy_static! {
    static ref VIDEO_ENCODER: Mutex<Option<ffmpeg::codec::Video>> = Mutex::new(None);
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

    let mut context = ffmpeg::format::output(&"/home/yalter/test.mp4")
        .chain_err(|| "could not create the output context")?;

    let mut encoder = {
        let mut stream = context.add_stream(codec)
            .chain_err(|| "could not add the video stream")?;

        let mut encoder = stream.codec().encoder().video()
            .chain_err(|| "could not retrieve the video encoder")?;

        encoder.set_width(640);
        encoder.set_height(360);
        encoder.set_time_base((1, 60));

        if let Some(mut formats) = codec.formats() {
            encoder.set_format(formats.next().unwrap());
        } else {
            encoder.set_format(ffmpeg::format::Pixel::YUV420P);
        }

        let encoder = encoder.open_as(codec).chain_err(|| "could not open the video encoder")?;
        stream.set_parameters(&encoder);

        stream.set_time_base((1, 60));

        encoder
    };

    context.write_header()
        .chain_err(|| "could not write the header")?;

    let stream_time_base = context.stream(0).unwrap().time_base();

    let mut converter = ffmpeg::software::converter((640, 360),
                                                ffmpeg::format::Pixel::RGB24,
                                                encoder.format())
        .chain_err(|| "could not get the color conversion context")?;

    {
        let mut frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::RGB24, 640, 360);
        let mut output_frame = ffmpeg::frame::Video::new(encoder.format(), 640, 360);

        for i in 0..255 {
            {
                let mut y = frame.plane_mut::<(u8, u8, u8)>(0);

                for j in 0..y.len() {
                    y[j] = (255, i as u8, 255 - i as u8);
                }
            }

            converter.run(&frame, &mut output_frame)
                .chain_err(|| "could not convert the frame to the correct format")?;

            output_frame.set_pts(Some(i));

            let mut packet = ffmpeg::Packet::empty();
            if encoder.encode(&output_frame, &mut packet).chain_err(|| "could not encode the frame")? {
                packet.rescale_ts((1, 60), stream_time_base);

                packet.write_interleaved(&mut context)
                    .chain_err(|| "could not write the packet")?;
            }
        }

        let mut packet = ffmpeg::Packet::empty();
        while encoder.flush(&mut packet).chain_err(|| "could not get the packet")? {
            packet.rescale_ts((1, 60), stream_time_base);

            packet.write_interleaved(&mut context)
                .chain_err(|| "could not write the packet")?;
        }
    }

    context.write_trailer()
        .chain_err(|| "could not write the trailer")?;

    Ok(())
}
