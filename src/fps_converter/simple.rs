use ffmpeg::format;

use super::*;
use capture;
use hooks::hw::FrameCapture;

/// Simple FPS converter which drops and duplicates frames to get constant FPS output.
pub struct SimpleConverter {
    /// Difference, in video frames, between how much time passed in-game and how much video we
    /// output.
    remainder: f64,

    /// The target time_base.
    time_base: f64,
}

impl SimpleConverter {
    #[inline]
    pub fn new(time_base: f64) -> Self {
        assert!(time_base > 0f64);

        Self {
            remainder: 0f64,
            time_base,
        }
    }
}

impl FPSConverter for SimpleConverter {
    fn time_passed<F>(&mut self, engine: &Engine, frametime: f64, capture: F)
    where
        F: FnOnce(&Engine) -> FrameCapture,
    {
        assert!(frametime >= 0.0f64);

        self.remainder += frametime / self.time_base;

        // Push this frame as long as it takes up the most of the video frame.
        // Remainder is > -0.5 at all times.
        let frames = (self.remainder + 0.5) as usize;
        self.remainder -= frames as f64;

        if frames > 0 {
            let frame_capture = capture(engine);

            let (w, h) = hw::get_resolution(engine);
            let mut buf = capture::get_buffer(engine, (w, h));

            match frame_capture {
                FrameCapture::OpenGL(read_pixels) => {
                    buf.set_format(format::Pixel::RGB24);
                    read_pixels(engine, (w, h), buf.as_mut_slice());
                }

                FrameCapture::OpenCL(ocl_gl_texture) => {
                    hw::read_ocl_image_into_buf(engine, ocl_gl_texture.as_ref(), &mut buf);
                }
            }

            capture::capture(engine, buf, frames);
        }
    }
}
