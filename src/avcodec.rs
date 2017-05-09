use ffmpeg_sys;
use std::ffi::{ CStr, CString };

use errors::*;

pub struct Codec {
    ptr: *mut ffmpeg_sys::AVCodec,
}

unsafe impl Send for Codec {}
unsafe impl Sync for Codec {}

pub struct PixelFormats<'a> {
    codec: &'a Codec,
    index: isize,
}

impl Codec {
    pub fn description(&self) -> String {
        unsafe {
            CStr::from_ptr((*self.ptr).long_name).to_string_lossy().into_owned()
        }
    }

    pub fn kind(&self) -> ffmpeg_sys::AVMediaType {
        unsafe {
            (*self.ptr).kind
        }
    }

    pub fn is_video(&self) -> bool {
        self.kind() == ffmpeg_sys::AVMediaType::AVMEDIA_TYPE_VIDEO
    }

    pub fn pixel_formats<'a>(&'a self) -> Option<PixelFormats<'a>> {
        unsafe {
            if (*self.ptr).pix_fmts.is_null() {
                None
            } else {
                Some(PixelFormats::new(&self))
            }
        }
    }
}

impl<'a> PixelFormats<'a> {
    fn new(codec: &'a Codec) -> Self {
        Self {
            codec,
            index: 0,
        }
    }
}

impl<'a> Iterator for PixelFormats<'a> {
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
