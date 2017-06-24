use ffmpeg::format;
use gl;
use gl::types::*;

use super::*;
use capture;
use hooks::hw::FrameCapture;

pub struct SimpleConverter {
    /// Difference, in video frames, between how much time passed in-game and how much video we
    /// output
    remainder: f64,

    /// The target time_base.
    time_base: f64,

    /// How many times the current frame should be output.
    frames: usize,
}

impl SimpleConverter {
    pub fn new(time_base: f64) -> Self {
        assert!(time_base > 0f64);

        Self {
            remainder: 0f64,
            time_base,
            frames: 0,
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
        self.frames = (self.remainder + 0.5) as usize;
        self.remainder -= self.frames as f64;

        if self.frames > 0 {
            let frame_capture = capture(engine);

            let (w, h) = hw::get_resolution(engine);
            let mut buf = capture::get_buffer(engine, (w, h));

            match frame_capture {
                FrameCapture::OpenGL => {
                    buf.set_format(format::Pixel::RGB24);

                    unsafe {
                        // Our buffer expects 1-byte alignment.
                        gl::PixelStorei(gl::PACK_ALIGNMENT, 1);

                        // Get the pixels!
                        gl::ReadPixels(0,
                                       0,
                                       w as GLsizei,
                                       h as GLsizei,
                                       gl::RGB,
                                       gl::UNSIGNED_BYTE,
                                       buf.as_mut_slice().as_mut_ptr() as _);
                    }
                }

                FrameCapture::OpenCL(ocl_gl_texture) => {
                    hw::read_ocl_image_into_buf(engine, &ocl_gl_texture, &mut buf);
                }
            }

            capture::capture(engine, buf, self.frames);
        }
    }
}
