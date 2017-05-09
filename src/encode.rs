use ffmpeg;

use errors::*;

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
        } else {
            engine.con_print(&format!("Invalid encoder type '{}'\n", encoder_name));
        }
    } else {
        engine.con_print(&format!("Unknown encoder '{}'\n", encoder_name));
    }
});
