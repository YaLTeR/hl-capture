use error_chain::ChainedError;
use ffmpeg_sys;
use std::sync::RwLock;

use avcodec;
use errors::*;

lazy_static! {
    static ref VIDEO_ENCODER: RwLock<Option<avcodec::Codec>> = RwLock::new(None);
}

/// Initialize the encoding stuff. Should be called once.
pub fn initialize() -> Result<()> {
    avcodec::initialize();

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
    match avcodec::find_encoder_by_name(&encoder_name) {
        Ok(Some(encoder)) => {
            if encoder.is_video() {
                let mut buf = String::new();

                buf.push_str(&format!("Encoder: {}\n", encoder_name));
                buf.push_str(&format!("Description: {}\n", encoder.description()));
                buf.push_str("Pixel formats: ");

                if let Some(formats) = encoder.pixel_formats() {
                    buf.push_str(&format!("{:?}\n", formats.collect::<Vec<_>>()));
                } else {
                    buf.push_str("any\n");
                }

                engine.con_print(&buf);

                *VIDEO_ENCODER.write().unwrap() = Some(encoder);
            } else {
                engine.con_print(&format!("Invalid encoder type '{}'\n", encoder_name));
            }
        }

        Ok(None) => {
            engine.con_print(&format!("Unknown encoder '{}'\n", encoder_name));
        },

        Err(ref e) => {
            engine.con_print(&format!("{}", e.display()));
        }
    }
});

command!(cap_test_video_output, |engine| {
    if let Err(ref e) = test_video_output() {
        engine.con_print(&format!("{}", e.display()));
        return;
    }

    engine.con_print("Done!\n");
});

fn test_video_output() -> Result<()> {
    use libc::EAGAIN;
    use std::ffi::CString;
    use std::ptr;

    let codec = *VIDEO_ENCODER.read().unwrap();
    ensure!(codec.is_some(), "codec is None");
    let codec = codec.unwrap();

    let octx = avcodec::OutputContext::new("/home/yalter/text.mp4")
        .chain_err(|| "unable to get the output context")?;

    // add stream
    // ==== TODO
    let stream = unsafe {
        ffmpeg_sys::avformat_new_stream(octx.ptr, ptr::null())
    };

    ensure!(!stream.is_null(), "unable to allocate the output stream");
    // /==== TODO

    let mut context = codec.context()
        .chain_err(|| "unable to get the codec context")?;

    context.set_width(640);
    context.set_height(360);
    context.set_time_base(&(1, 60).into());
    context.set_pixel_format(ffmpeg_sys::AVPixelFormat::AV_PIX_FMT_YUV420P);

    // Get ready for encoding.
    let context = context.open()
        .chain_err(|| "could not open context for encoding")?;

    // parameters from context into stream
    // ==== TODO
    let rv = unsafe {
        ffmpeg_sys::avcodec_parameters_from_context((*stream).codecpar, context.context.ptr)
    };

    ensure!(rv >= 0, "could not copy encoder parameters to the output stream");
    // /==== TODO

    // if (ofmt_ctx->oformat->flags & AVFMT_GLOBALHEADER)
    //     enc_ctx->flags |= AV_CODEC_FLAG_GLOBAL_HEADER;
    // ==== TODO
    let flags = unsafe {
        (*(*octx.ptr).oformat).flags
    };

    if (flags & ffmpeg_sys::AVFMT_GLOBALHEADER) != 0 {
        unsafe {
            (*context.context.ptr).flags |= ffmpeg_sys::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
        }
    }
    // /==== TODO

    // set stream time_base
    // ==== TODO
    unsafe {
        (*stream).time_base = context.context.time_base().into();
    }

    unsafe {
        println!("right after setting up: encoder time base: {:?}, stream time base: {:?}", context.context.time_base(), (*stream).time_base);
    }
    // /==== TODO

    // maybe av_dump_format for debug log purposes
    // ==== TODO
    unsafe {
        ffmpeg_sys::av_dump_format(octx.ptr, 0, CString::new("/home/yalter/test.mp4").unwrap().as_ptr(), 1);
    }
    // /==== TODO

    // if (!(ofmt_ctx->oformat->flags & AVFMT_NOFILE)) {
    //     ret = avio_open(&ofmt_ctx->pb, filename, AVIO_FLAG_WRITE);
    //     if (ret < 0) {
    //         av_log(NULL, AV_LOG_ERROR, "Could not open output file '%s'", filename);
    //         return ret;
    //     }
    // }
    // ==== TODO
    if (flags & ffmpeg_sys::AVFMT_NOFILE) == 0 {
        let rv = unsafe {
            ffmpeg_sys::avio_open(&mut (*octx.ptr).pb, CString::new("/home/yalter/test.mp4").unwrap().as_ptr(), ffmpeg_sys::AVIO_FLAG_WRITE)
        };

        ensure!(rv >= 0, "unable to open the output file for writing");
    }

    unsafe {
        println!("after avio_open: encoder time base: {:?}, stream time base: {:?}", context.context.time_base(), (*stream).time_base);
    }
    // /==== TODO

    // avformat_write_header
    // ==== TODO
    let rv = unsafe {
        ffmpeg_sys::avformat_write_header(octx.ptr, ptr::null_mut())
    };

    ensure!(rv >= 0, "unable to write header to the output file");

    unsafe {
        println!("after avformat_write_header: encoder time base: {:?}, stream time base: {:?}", context.context.time_base(), (*stream).time_base);
    }
    // /==== TODO

    // write frames:
    //     make a frame, fill it with data
    //     avcodec_send_frame
    //     avcodec_receive_packet loop until AVERROR(EAGAIN)
    // ==== TODO
    let mut packet = unsafe {
        ffmpeg_sys::av_packet_alloc()
    };
    ensure!(!packet.is_null(), "unsable to allocate a packet");

    unsafe {
        println!("right before encoding: encoder time base: {:?}, stream time base: {:?}", context.context.time_base(), (*stream).time_base);
    }

    for i in 0..60 {
        let mut frame = avcodec::Frame::new(ffmpeg_sys::AV_PIX_FMT_YUV420P, 640, 360)
            .chain_err(|| "could not allocate a frame")?;

        unsafe {
            let linesize = (*frame.ptr).linesize;
            let data = (*frame.ptr).data;

            for y in 0..360 {
                for x in 0..640 {
                    *data[0].offset((y * linesize[0] + x) as isize) = (x + y) as u8;
                }
            }

            for y in 0..360/2 {
                for x in 0..640/2 {
                    *data[1].offset((y * linesize[1] + x) as isize) = (x + y) as u8;
                    *data[2].offset((y * linesize[2] + x) as isize) = (x + y) as u8;
                }
            }

            (*frame.ptr).pts = i;
        }

        // send to encoder
        let rv = unsafe {
            ffmpeg_sys::avcodec_send_frame(context.context.ptr, frame.ptr)
        };
        ensure!(rv == 0, "error sending a frame for encoding");

        loop {
            let rv = unsafe {
                ffmpeg_sys::avcodec_receive_packet(context.context.ptr, packet)
            };
            ensure!(rv == 0 || rv == ffmpeg_sys::AVERROR(EAGAIN), "error receiving a packet");

            if rv == ffmpeg_sys::AVERROR(EAGAIN) {
                break;
            }

            unsafe {
                ffmpeg_sys::av_packet_rescale_ts(packet, context.context.time_base().into(), (*stream).time_base);
            }

            let rv = unsafe {
                ffmpeg_sys::av_interleaved_write_frame(octx.ptr, packet)
            };
            ensure!(rv == 0, "error writing the packet to the output file");
        }
    }
    // /==== TODO

    // flush:
    //     avcodec_send_frame(null)
    //     avcodec_receive_packet loop until AVERROR_EOF
    // ==== TODO
    let rv = unsafe {
        ffmpeg_sys::avcodec_send_frame(context.context.ptr, ptr::null())
    };
    ensure!(rv == 0, "error flushing the packets");

    loop {
        let rv = unsafe {
            ffmpeg_sys::avcodec_receive_packet(context.context.ptr, packet)
        };
        ensure!(rv == 0 || rv == ffmpeg_sys::AVERROR_EOF, "error receiving a packet during flushing");

        if rv == ffmpeg_sys::AVERROR_EOF {
            break;
        }

        unsafe {
            ffmpeg_sys::av_packet_rescale_ts(packet, context.context.time_base().into(), (*stream).time_base);
        }

        let rv = unsafe {
            ffmpeg_sys::av_interleaved_write_frame(octx.ptr, packet)
        };
        ensure!(rv == 0, "error writing the packet to the output file during flushing");
    }

    unsafe {
        ffmpeg_sys::av_packet_free(&mut packet);
    }
    // /==== TODO

    // av_write_trailer
    // ==== TODO
    unsafe {
        ffmpeg_sys::av_write_trailer(octx.ptr);
    }

    if (flags & ffmpeg_sys::AVFMT_NOFILE) == 0 {
        unsafe {
            ffmpeg_sys::avio_closep(&mut (*octx.ptr).pb);
        }
    }
    // /==== TODO

    Ok(())
}
