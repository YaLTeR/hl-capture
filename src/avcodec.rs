use ffmpeg_sys;
use libc::*;
use std::cmp;
use std::ffi::{ CStr, CString };
use std::ops::Deref;

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
    ptr: *mut ffmpeg_sys::AVCodecContext,
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

        Ok(Context { ptr })
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
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            ffmpeg_sys::avcodec_free_context(&mut self.ptr);
        }
    }
}

pub fn initialize() {
    unsafe {
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
