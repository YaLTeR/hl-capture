#![allow(dead_code)]
use ffmpeg_sys;
use libc::*;
use std::cmp;
use std::ffi::{ CStr, CString };
use std::ops::Deref;
use std::ptr;

use errors::*;

#[derive(Debug, Clone, Copy)]
pub struct Rational {
    pub numerator: i32,
    pub denominator: i32,
}

#[derive(Clone, Copy)]
pub struct Codec {
    ptr: *mut ffmpeg_sys::AVCodec,
}

unsafe impl Send for Codec {}
unsafe impl Sync for Codec {}

pub struct PixelFormats {
    codec: Codec,
    index: isize,
}

pub struct Context {
    // The Codec which was used to initialize this Context.
    codec: Codec,
    ptr: *mut ffmpeg_sys::AVCodecContext,
}

pub struct OpenContext {
    context: Context,
}

pub struct Frame {
    ptr: *mut ffmpeg_sys::AVFrame,
}

pub struct OutputContext {
    ptr: *mut ffmpeg_sys::AVFormatContext,
}

impl From<(i32, i32)> for Rational {
    fn from((numerator, denominator): (i32, i32)) -> Self {
        Self {
            numerator,
            denominator,
        }
    }
}

impl From<ffmpeg_sys::AVRational> for Rational {
    fn from(rational: ffmpeg_sys::AVRational) -> Self {
        Self {
            numerator: rational.num,
            denominator: rational.den,
        }
    }
}

impl From<Rational> for ffmpeg_sys::AVRational {
    fn from(rational: Rational) -> Self {
        Self {
            num: rational.numerator,
            den: rational.denominator,
        }
    }
}

impl Codec {
    pub fn description(self) -> String {
        unsafe {
            CStr::from_ptr((*self.ptr).long_name).to_string_lossy().into_owned()
        }
    }

    #[inline]
    pub fn kind(self) -> ffmpeg_sys::AVMediaType {
        unsafe {
            (*self.ptr).kind
        }
    }

    #[inline]
    pub fn is_video(self) -> bool {
        self.kind() == ffmpeg_sys::AVMediaType::AVMEDIA_TYPE_VIDEO
    }

    pub fn pixel_formats(self) -> Option<PixelFormats> {
        unsafe {
            if (*self.ptr).pix_fmts.is_null() {
                None
            } else {
                Some(PixelFormats::new(self))
            }
        }
    }

    pub fn context(self) -> Result<Context> {
        let ptr = unsafe {
            ffmpeg_sys::avcodec_alloc_context3(self.ptr)
        };

        ensure!(!ptr.is_null(), "unable to allocate the codec context");

        Ok(Context {
            codec: self,
            ptr,
        })
    }
}

impl PixelFormats {
    fn new(codec: Codec) -> Self {
        Self {
            codec,
            index: 0,
        }
    }
}

impl Iterator for PixelFormats {
    type Item = ffmpeg_sys::AVPixelFormat;

    fn next(&mut self) -> Option<Self::Item> {
        let format = unsafe {
            *(*self.codec.ptr).pix_fmts.offset(self.index)
        };

        if format == ffmpeg_sys::AVPixelFormat::AV_PIX_FMT_NONE {
            None
        } else {
            self.index += 1;
            Some(format)
        }
    }
}

impl Context {
    #[inline]
    pub fn width(&self) -> u32 {
        unsafe {
            cmp::max(0, (*self.ptr).width) as u32
        }
    }

    #[inline]
    pub fn set_width(&mut self, width: u32) {
        unsafe {
            (*self.ptr).width = cmp::min(width, c_int::max_value() as u32) as c_int;
        }
    }

    #[inline]
    pub fn height(&self) -> u32 {
        unsafe {
            cmp::max(0, (*self.ptr).height) as u32
        }
    }

    #[inline]
    pub fn set_height(&mut self, height: u32) {
        unsafe {
            (*self.ptr).height = cmp::min(height, c_int::max_value() as u32) as c_int;
        }
    }

    #[inline]
    pub fn time_base(&self) -> Rational {
        unsafe {
            (*self.ptr).time_base.into()
        }
    }

    #[inline]
    pub fn set_time_base(&mut self, time_base: &Rational) {
        unsafe {
            (*self.ptr).time_base = (*time_base).into();
        }
    }

    #[inline]
    pub fn pixel_format(&self) -> ffmpeg_sys::AVPixelFormat {
        unsafe {
            (*self.ptr).pix_fmt
        }
    }

    #[inline]
    pub fn set_pixel_format(&mut self, pixel_format: ffmpeg_sys::AVPixelFormat) {
        unsafe {
            (*self.ptr).pix_fmt = pixel_format;
        }
    }

    pub fn open(self) -> Result<OpenContext> {
        let rv = unsafe {
            ffmpeg_sys::avcodec_open2(self.ptr, self.codec.ptr, ptr::null_mut())
        };

        ensure!(rv == 0, "error opening context");

        Ok(OpenContext { context: self })
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            ffmpeg_sys::avcodec_free_context(&mut self.ptr);
        }
    }
}

impl Frame {
    pub fn new(pixel_format: ffmpeg_sys::AVPixelFormat,
               width: u32,
               height: u32) -> Result<Self> {
        let ptr = unsafe {
            ffmpeg_sys::av_frame_alloc()
        };

        ensure!(!ptr.is_null(), "unable to allocate a frame");

        unsafe {
            (*ptr).format = pixel_format as c_int;
            (*ptr).width = cmp::min(width, c_int::max_value() as u32) as c_int;
            (*ptr).height = cmp::min(height, c_int::max_value() as u32) as c_int;
        }

        Ok(Self { ptr })
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe {
            ffmpeg_sys::av_frame_free(&mut self.ptr);
        }
    }
}

impl OutputContext {
    pub fn new(filename: &str) -> Result<Self> {
        let filename = CString::new(filename)
            .chain_err(|| "unable to convert filename to a CString")?;

        let mut ptr = ptr::null_mut();

        let rv = unsafe {
            ffmpeg_sys::avformat_alloc_output_context2(&mut ptr,
                                                       ptr::null_mut(),
                                                       ptr::null_mut(),
                                                       filename.as_ptr())
        };

        // TODO: check and report the error code.
        ensure!(rv >= 0, "unable to allocate the output context");

        Ok(Self { ptr })
    }
}

impl Drop for OutputContext {
    fn drop(&mut self) {
        unsafe {
            ffmpeg_sys::avformat_free_context(self.ptr);
        }
    }
}

pub fn initialize() {
    unsafe {
        ffmpeg_sys::av_register_all();
        ffmpeg_sys::avcodec_register_all();
    }
}

pub fn find_encoder_by_name(name: &str) -> Result<Option<Codec>> {
    let name = CString::new(name).chain_err(|| "could not convert name to CString")?;

    let codec = unsafe {
        ffmpeg_sys::avcodec_find_encoder_by_name(name.as_ptr())
    };

    if codec.is_null() {
        Ok(None)
    } else {
        Ok(Some(Codec { ptr: codec }))
    }
}
