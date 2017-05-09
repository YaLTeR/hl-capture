use error_chain::ChainedError;
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
    let codec = *VIDEO_ENCODER.read().unwrap();
    ensure!(codec.is_some(), "codec is None");
    let codec = codec.unwrap();

    let context = codec.context()
        .chain_err(|| "unable to get the codec context")?;

    Ok(())
}
