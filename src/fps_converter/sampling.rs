use ffmpeg::format;
use gl;
use gl::types::*;
use ocl;

use super::*;
use capture;
use hooks::hw::FrameCapture;

pub struct SamplingConverter {
    /// Difference, in video frames, between how much time passed in-game and how much video we
    /// output.
    remainder: f64,

    /// The target time_base.
    time_base: f64,

    /// Data with a destructor.
    private: *mut SamplingConverterPrivate,
}

struct SamplingConverterPrivate {
    /// OpenCL buffer images.
    ocl_buffers: [ocl::Image<u8>; 2],

    /// OpenCL output image.
    ocl_output_image: ocl::Image<u8>,

    /// Index of the last written to OpenCL buffer.
    ocl_current_buffer_index: usize,
}

impl SamplingConverter {
    pub fn new(engine: &Engine, time_base: f64, video_resolution: (u32, u32)) -> Self {
        assert!(time_base > 0f64);

        Self {
            remainder: 0f64,
            time_base,
            private: Box::into_raw(Box::new(SamplingConverterPrivate::new(engine,
                                                                          video_resolution))),
        }
    }

    pub fn free(&mut self) {
        drop(unsafe { Box::from_raw(self.private) });
    }
}

impl FPSConverter for SamplingConverter {
    fn time_passed<F>(&mut self, engine: &Engine, frametime: f64, capture: F)
    where
        F: FnOnce(&Engine) -> FrameCapture,
    {
        assert!(frametime >= 0.0f64);

        let frame_capture = capture(engine);
        let mut private = unsafe { self.private.as_mut().unwrap() };

        let old_remainder = self.remainder;
        self.remainder += frametime / self.time_base;

        if self.remainder < 1f64 {
            let weight = self.remainder - old_remainder;

            match frame_capture {
                FrameCapture::OpenGL => {
                    unimplemented!();
                }

                FrameCapture::OpenCL(ocl_gl_texture) => {
                    weighted_image_add(engine,
                                       ocl_gl_texture.as_ref(),
                                       private.src_buffer(),
                                       private.dst_buffer(),
                                       weight as f32);

                    private.switch_buffer_index();
                }
            }
        } else {
            let weight = 1f64 - old_remainder;

            match frame_capture {
                FrameCapture::OpenGL => {
                    unimplemented!();
                }

                FrameCapture::OpenCL(ocl_gl_texture) => {
                    weighted_image_add(engine,
                                       ocl_gl_texture.as_ref(),
                                       private.src_buffer(),
                                       private.output_image(),
                                       weight as f32);

                    fill_with_black(engine, private.dst_buffer());

                    private.switch_buffer_index();

                    // Output the frame.
                    let (w, h) = hw::get_resolution(engine);
                    let mut buf = capture::get_buffer(engine, (w, h));
                    hw::read_ocl_image_into_buf(engine, private.output_image(), &mut buf);
                    capture::capture(engine, buf, 1);

                    self.remainder -= 1f64;

                    // Output it more times if needed.
                    let additional_frames = self.remainder as usize;
                    if additional_frames > 0 {
                        let mut buf = capture::get_buffer(engine, (w, h));
                        hw::read_ocl_image_into_buf(engine, ocl_gl_texture.as_ref(), &mut buf);
                        capture::capture(engine, buf, additional_frames);

                        self.remainder -= additional_frames as f64;
                    }

                    // Add the remaining image into the buffer.
                    weighted_image_add(engine,
                                       ocl_gl_texture.as_ref(),
                                       private.src_buffer(),
                                       private.dst_buffer(),
                                       self.remainder as f32);
                    private.switch_buffer_index();
                }
            }
        }


        // let (w, h) = hw::get_resolution(engine);
        // let mut buf = capture::get_buffer(engine, (w, h));
        //
        // match frame_capture {
        //     FrameCapture::OpenGL => {
        //         buf.set_format(format::Pixel::RGB24);
        //
        //         unsafe {
        //             // Our buffer expects 1-byte alignment.
        //             gl::PixelStorei(gl::PACK_ALIGNMENT, 1);
        //
        //             // Get the pixels!
        //             gl::ReadPixels(0,
        //                            0,
        //                            w as GLsizei,
        //                            h as GLsizei,
        //                            gl::RGB,
        //                            gl::UNSIGNED_BYTE,
        //                            buf.as_mut_slice().as_mut_ptr() as _);
        //         }
        //     }
        //
        //     FrameCapture::OpenCL(ocl_gl_texture) => {
        //         hw::read_ocl_image_into_buf(engine, &ocl_gl_texture, &mut buf);
        //     }
        // }
        //
        // capture::capture(engine, buf, frames);
    }
}

impl SamplingConverterPrivate {
    fn new(engine: &Engine, (w, h): (u32, u32)) -> Self {
        let pro_que = hw::get_pro_que(engine).expect("sampling currently requires OpenCL");

        let rv = Self {
            ocl_buffers: [
                hw::build_ocl_image(engine,
                                    &pro_que,
                                    ocl::MemFlags::new().read_write().host_no_access(),
                                    ocl::enums::ImageChannelDataType::Float,
                                    (w, h).into())
                .expect("building an OpenCL image"),
                hw::build_ocl_image(engine,
                                    &pro_que,
                                    ocl::MemFlags::new().read_write().host_no_access(),
                                    ocl::enums::ImageChannelDataType::Float,
                                    (w, h).into())
                .expect("building an OpenCL image"),
            ],
            ocl_output_image: hw::build_ocl_image(engine,
                                                  &pro_que,
                                                  ocl::MemFlags::new()
                                                      .read_write()
                                                      .host_read_only(),
                                                  ocl::enums::ImageChannelDataType::Float,
                                                  (w, h).into())
                              .expect("building an OpenCL image"),
            ocl_current_buffer_index: 0,
        };

        fill_with_black(engine, rv.src_buffer());

        rv
    }

    fn src_buffer(&self) -> &ocl::Image<u8> {
        &self.ocl_buffers[self.ocl_current_buffer_index]
    }

    fn dst_buffer(&self) -> &ocl::Image<u8> {
        &self.ocl_buffers[self.ocl_current_buffer_index ^ 1]
    }

    fn output_image(&self) -> &ocl::Image<u8> {
        &self.ocl_output_image
    }

    fn switch_buffer_index(&mut self) {
        self.ocl_current_buffer_index ^= 1;
    }
}

fn weighted_image_add(engine: &Engine,
                      src: &ocl::Image<u8>,
                      buf: &ocl::Image<u8>,
                      dst: &ocl::Image<u8>,
                      weight: f32) {
    let pro_que = hw::get_pro_que(engine).unwrap();

    let kernel = pro_que.create_kernel("weighted_image_add")
                        .unwrap()
                        .gws(src.dims())
                        .arg_img(src)
                        .arg_img(buf)
                        .arg_img(dst)
                        .arg_scl(weight);

    kernel.enq().expect("sampling kernel enq()");
}

fn fill_with_black(engine: &Engine, image: &ocl::Image<u8>) {
    let pro_que = hw::get_pro_que(engine).unwrap();

    let kernel = pro_que.create_kernel("fill_with_black")
                        .unwrap()
                        .gws(image.dims())
                        .arg_img(image);

    kernel.enq().expect("sampling kernel enq()");
}
