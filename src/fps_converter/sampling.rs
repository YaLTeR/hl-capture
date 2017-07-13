use ffmpeg::format;
use ocl::{self, OclPrm};

use super::*;
use capture;
use hooks::hw::FrameCapture;
use manual_free::ManualFree;

/// Resampling FPS converter which averages input frames for smooth motion.
pub struct SamplingConverter {
    /// Difference, in video frames, between how much time passed in-game and how much video we
    /// output.
    remainder: f64,

    /// The target time_base.
    time_base: f64,

    /// Data with a destructor, wrapped so `SamplingConverter` can be put in a static variable.
    private: ManualFree<SamplingConverterPrivate>,
}

/// Data with a destructor.
struct SamplingConverterPrivate {
    /// Data used for OpenCL operations.
    ///
    /// This is Some(None) if OpenCL is unavailable and None during an engine restart.
    ocl_runtime_data: Option<Option<OclRuntimeData>>,

    /// Pixels from the buffer are stored here when the engine restarts.
    ocl_backup_buffer: Option<Vec<ocl::prm::Float>>,

    /// The video resolution.
    video_resolution: (u32, u32),

    /// The OpenGL sampling buffer.
    gl_sampling_buffer: Vec<f32>,

    /// The OpenGL read buffer.
    gl_read_buffer: Vec<u8>,
}

/// Data used at runtime by OpenCL sampling.
struct OclRuntimeData {
    /// Buffer images.
    ocl_buffers: [ocl::Image<ocl::prm::Float>; 2],

    /// Output image.
    ocl_output_image: ocl::Image<ocl::prm::Float>,

    /// Index of the last written to OpenCL buffer.
    ocl_current_buffer_index: usize,
}

impl SamplingConverter {
    #[inline]
    pub fn new(engine: &Engine, time_base: f64, video_resolution: (u32, u32)) -> Self {
        assert!(time_base > 0f64);

        Self {
            remainder: 0f64,
            time_base,
            private: ManualFree::new(SamplingConverterPrivate::new(engine, video_resolution)),
        }
    }

    /// This should be called before an engine restart.
    #[inline]
    pub fn backup_and_free_ocl_data(&mut self, engine: &Engine) {
        self.private.backup_and_free_ocl_data(engine);
    }

    #[inline]
    pub fn free(&mut self) {
        self.private.free();
    }
}

impl FPSConverter for SamplingConverter {
    fn time_passed<F>(&mut self, engine: &Engine, frametime: f64, capture: F)
    where
        F: FnOnce(&Engine) -> FrameCapture,
    {
        assert!(frametime >= 0.0f64);

        let frame_capture = capture(engine);

        let old_remainder = self.remainder;
        self.remainder += frametime / self.time_base;

        let exposure = capture::get_capture_parameters(engine).sampling_exposure;

        if self.remainder <= (1f64 - exposure) {
            // Do nothing.
        } else if self.remainder < 1f64 {
            let weight = (self.remainder - old_remainder.max(1f64 - exposure)) * (1f64 / exposure);

            match frame_capture {
                FrameCapture::OpenGL(read_pixels) => {
                    let (w, h) = self.private.video_resolution;
                    self.private.gl_read_buffer.resize((w * h * 3) as usize, 0);
                    self.private
                        .gl_sampling_buffer
                        .resize((w * h * 3) as usize, 0f32);

                    read_pixels(engine, (w, h), &mut self.private.gl_read_buffer);

                    let private: &mut SamplingConverterPrivate = &mut self.private;
                    weighted_image_add(&mut private.gl_sampling_buffer,
                                       &private.gl_read_buffer,
                                       weight as f32);
                }

                FrameCapture::OpenCL(ocl_gl_texture) => {
                    let ocl_data = self.private.get_ocl_data(engine).unwrap();

                    ocl_weighted_image_add(engine,
                                           ocl_gl_texture.as_ref(),
                                           ocl_data.src_buffer(),
                                           ocl_data.dst_buffer(),
                                           weight as f32);

                    ocl_data.switch_buffer_index();
                }
            }
        } else {
            let weight = (1f64 - old_remainder.max(1f64 - exposure)) * (1f64 / exposure);

            match frame_capture {
                FrameCapture::OpenGL(read_pixels) => {
                    let (w, h) = self.private.video_resolution;
                    self.private.gl_read_buffer.resize((w * h * 3) as usize, 0);
                    self.private
                        .gl_sampling_buffer
                        .resize((w * h * 3) as usize, 0f32);

                    read_pixels(engine, (w, h), &mut self.private.gl_read_buffer);

                    let mut buf = capture::get_buffer(engine, (w, h));
                    buf.set_format(format::Pixel::RGB24);
                    weighted_image_add_to(&self.private.gl_sampling_buffer,
                                          &self.private.gl_read_buffer,
                                          buf.as_mut_slice(),
                                          weight as f32);
                    capture::capture(engine, buf, 1);

                    fill_with_black(&mut self.private.gl_sampling_buffer);

                    self.remainder -= 1f64;

                    // Output it more times if needed.
                    let additional_frames = self.remainder as usize;
                    if additional_frames > 0 {
                        let mut buf = capture::get_buffer(engine, (w, h));
                        buf.set_format(format::Pixel::RGB24);
                        buf.as_mut_slice()
                           .copy_from_slice(&self.private.gl_read_buffer);
                        capture::capture(engine, buf, additional_frames);

                        self.remainder -= additional_frames as f64;
                    }

                    // Add the remaining image into the buffer.
                    if self.remainder > (1f64 - exposure) {
                        let private: &mut SamplingConverterPrivate = &mut self.private;
                        weighted_image_add(&mut private.gl_sampling_buffer,
                                           &private.gl_read_buffer,
                                           ((self.remainder - (1f64 - exposure)) *
                                                (1f64 / exposure)) as
                                               f32);
                    }
                }

                FrameCapture::OpenCL(ocl_gl_texture) => {
                    let ocl_data = self.private.get_ocl_data(engine).unwrap();

                    ocl_weighted_image_add(engine,
                                           ocl_gl_texture.as_ref(),
                                           ocl_data.src_buffer(),
                                           ocl_data.output_image(),
                                           weight as f32);

                    ocl_fill_with_black(engine, ocl_data.dst_buffer());

                    ocl_data.switch_buffer_index();

                    // Output the frame.
                    let (w, h) = hw::get_resolution(engine);
                    let mut buf = capture::get_buffer(engine, (w, h));
                    hw::read_ocl_image_into_buf(engine, ocl_data.output_image(), &mut buf);
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
                    if self.remainder > (1f64 - exposure) {
                        ocl_weighted_image_add(engine,
                                               ocl_gl_texture.as_ref(),
                                               ocl_data.src_buffer(),
                                               ocl_data.dst_buffer(),
                                               ((self.remainder - (1f64 - exposure)) *
                                                    (1f64 / exposure)) as
                                                   f32);
                        ocl_data.switch_buffer_index();
                    }
                }
            }
        }
    }
}

impl SamplingConverterPrivate {
    #[inline]
    fn new(engine: &Engine, video_resolution: (u32, u32)) -> Self {
        Self {
            ocl_runtime_data: Some(OclRuntimeData::new(engine, video_resolution)),
            ocl_backup_buffer: None,
            video_resolution,
            gl_sampling_buffer: Vec::new(),
            gl_read_buffer: Vec::new(),
        }
    }

    #[inline]
    fn get_ocl_data(&mut self, engine: &Engine) -> Option<&mut OclRuntimeData> {
        if self.ocl_runtime_data.is_none() {
            self.restore_ocl_data(engine);
        }

        self.ocl_runtime_data.as_mut().unwrap().as_mut()
    }

    /// This should be called before an engine restart.
    fn backup_and_free_ocl_data(&mut self, engine: &Engine) {
        let set_to_none = if let Some(Some(ref ocl_data)) = self.ocl_runtime_data {
            // Copy the src buffer into the output image.
            ocl_weighted_image_add(engine,
                                   ocl_data.dst_buffer(),
                                   ocl_data.src_buffer(),
                                   ocl_data.output_image(),
                                   0f32);

            let image = ocl_data.output_image();

            let mut backup_buffer = Vec::with_capacity(image.element_count());
            backup_buffer.resize(image.element_count(), 0f32.into());

            image.read(&mut backup_buffer).enq().expect("image.read()");

            self.ocl_backup_buffer = Some(backup_buffer);

            true
        } else {
            false
        };

        if set_to_none {
            self.ocl_runtime_data = None;
        }
    }

    /// This should be called after an engine restart.
    fn restore_ocl_data(&mut self, engine: &Engine) {
        if self.ocl_runtime_data.is_some() {
            panic!("tried to restore already existing OpenCL data");
        }

        let ocl_data = OclRuntimeData::new(engine, self.video_resolution)
            .expect("changing from fullscreen to windowed is not supported");

        let pro_que = hw::get_pro_que(engine).unwrap();
        let temp_image = hw::build_ocl_image(engine,
                                             pro_que,
                                             ocl::MemFlags::new().read_only().host_write_only(),
                                             ocl::enums::ImageChannelDataType::Float,
                                             self.video_resolution.into())
                         .expect("building an OpenCL image");

        let backup_buffer = self.ocl_backup_buffer.take().unwrap();
        temp_image.write(&backup_buffer)
                  .enq()
                  .expect("image.write()");

        // Copy the backup buffer into the src buffer.
        ocl_weighted_image_add(engine,
                               ocl_data.dst_buffer(),
                               &temp_image,
                               ocl_data.src_buffer(),
                               0f32);

        self.ocl_runtime_data = Some(Some(ocl_data));
    }
}

impl OclRuntimeData {
    fn new(engine: &Engine, (w, h): (u32, u32)) -> Option<Self> {
        hw::get_pro_que(engine).map(|pro_que| {
            let rv = Self {
                ocl_buffers: [
                    hw::build_ocl_image(engine,
                                        pro_que,
                                        ocl::MemFlags::new().read_write().host_no_access(),
                                        ocl::enums::ImageChannelDataType::Float,
                                        (w, h).into())
                    .expect("building an OpenCL image"),
                    hw::build_ocl_image(engine,
                                        pro_que,
                                        ocl::MemFlags::new().read_write().host_no_access(),
                                        ocl::enums::ImageChannelDataType::Float,
                                        (w, h).into())
                    .expect("building an OpenCL image"),
                ],
                ocl_output_image: hw::build_ocl_image(engine,
                                                      pro_que,
                                                      ocl::MemFlags::new()
                                                          .read_write()
                                                          .host_read_only(),
                                                      ocl::enums::ImageChannelDataType::Float,
                                                      (w, h).into())
                                  .expect("building an OpenCL image"),
                ocl_current_buffer_index: 0,
            };

            ocl_fill_with_black(engine, rv.src_buffer());

            rv
        })
    }

    #[inline]
    fn src_buffer(&self) -> &ocl::Image<ocl::prm::Float> {
        &self.ocl_buffers[self.ocl_current_buffer_index]
    }

    #[inline]
    fn dst_buffer(&self) -> &ocl::Image<ocl::prm::Float> {
        &self.ocl_buffers[self.ocl_current_buffer_index ^ 1]
    }

    #[inline]
    fn output_image(&self) -> &ocl::Image<ocl::prm::Float> {
        &self.ocl_output_image
    }

    #[inline]
    fn switch_buffer_index(&mut self) {
        self.ocl_current_buffer_index ^= 1;
    }
}

#[inline]
fn ocl_weighted_image_add<T: OclPrm, U: OclPrm, V: OclPrm>(engine: &Engine,
                                                           src: &ocl::Image<T>,
                                                           buf: &ocl::Image<U>,
                                                           dst: &ocl::Image<V>,
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

#[inline]
fn ocl_fill_with_black<T: OclPrm>(engine: &Engine, image: &ocl::Image<T>) {
    let pro_que = hw::get_pro_que(engine).unwrap();

    let kernel = pro_que.create_kernel("fill_with_black")
                        .unwrap()
                        .gws(image.dims())
                        .arg_img(image);

    kernel.enq().expect("sampling kernel enq()");
}

#[inline]
fn weighted_image_add(buf: &mut [f32], image: &[u8], weight: f32) {
    assert_eq!(buf.len(), image.len());

    for i in 0..buf.len() {
        buf[i] += image[i] as f32 * weight;
    }
}

#[inline]
fn weighted_image_add_to(buf: &[f32], image: &[u8], dst: &mut [u8], weight: f32) {
    assert_eq!(buf.len(), image.len());
    assert_eq!(buf.len(), dst.len());

    for i in 0..buf.len() {
        dst[i] = (buf[i] + image[i] as f32 * weight).round() as u8;
    }
}

#[inline]
fn fill_with_black(buf: &mut [f32]) {
    for i in 0..buf.len() {
        buf[i] = 0f32;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn fill_with_black_test() {
        let mut buf = [1f32, 2f32, 3f32, 4f32, 5f32];

        fill_with_black(&mut buf[..]);

        assert_eq!(buf, [0f32, 0f32, 0f32, 0f32, 0f32]);
    }

    #[test]
    fn weighted_image_add_test() {
        let mut buf = [1f32, 2f32, 3f32, 4f32, 5f32];
        let image = [10, 20, 30, 40, 50];

        weighted_image_add(&mut buf, &image, 0.5f32);

        assert_eq!(buf, [6f32, 12f32, 18f32, 24f32, 30f32]);
    }

    #[test]
    #[should_panic]
    fn weighted_image_add_len_mismatch_test() {
        let mut buf = [1f32, 2f32, 3f32, 4f32, 5f32];
        let image = [10, 20, 30, 40, 50, 60];

        weighted_image_add(&mut buf, &image, 0.5f32);
    }

    #[test]
    fn weighted_image_add_to_test() {
        let buf = [1f32, 2f32, 3f32, 4f32, 5f32];
        let image = [10, 20, 30, 40, 50];
        let mut dst = [5, 4, 3, 2, 1];

        weighted_image_add_to(&buf, &image, &mut dst, 0.5f32);

        assert_eq!(dst, [6, 12, 18, 24, 30]);
    }

    #[test]
    #[should_panic]
    fn weighted_image_add_to_len_mismatch_test() {
        let buf = [1f32, 2f32, 3f32, 4f32, 5f32];
        let image = [10, 20, 30, 40, 50, 60];
        let mut dst = [5, 4, 3, 2, 1];

        weighted_image_add_to(&buf, &image, &mut dst, 0.5f32);
    }

    #[test]
    #[should_panic]
    fn weighted_image_add_to_dst_len_mismatch_test() {
        let buf = [1f32, 2f32, 3f32, 4f32, 5f32];
        let image = [10, 20, 30, 40, 50];
        let mut dst = [5, 4, 3, 2, 1, 0];

        weighted_image_add_to(&buf, &image, &mut dst, 0.5f32);
    }
}
