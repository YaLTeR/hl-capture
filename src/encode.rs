use error_chain::ChainedError;
use ffmpeg_sys;
use std::sync::Mutex;

use errors::*;

lazy_static! {
}

/// Initialize the encoding stuff. Should be called once.
pub fn initialize() -> Result<()> {
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

});

command!(cap_test_video_output, |engine| {
});

fn test_video_output() -> Result<()> {
    Ok(())
}
