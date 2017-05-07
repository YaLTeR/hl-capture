#![allow(dead_code)]
use libc::*;
use std::ffi::{CStr, CString};
use std::ptr;

use errors::*;

pub struct Handle {
    ptr: *mut c_void,
}

unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            dlclose(self.ptr);
        }
    }
}

impl Handle {
    pub fn sym(&self, symbol: &str) -> Result<*mut c_void> {
        // Clear the previous error.
        unsafe {
            dlerror();
        }

        let symbol = CString::new(symbol)
            .chain_err(|| "unable to convert symbol to a CString")?;
        let ptr = unsafe { dlsym(self.ptr, symbol.as_ptr()) };

        let error = unsafe { dlerror() };
        if !error.is_null() {
            let error = unsafe { CStr::from_ptr(error).to_string_lossy() };
            bail!("dlsym failed with `{}`", error);
        }

        Ok(ptr)
    }
}

pub enum OpenTarget<'a> {
    MainProgram,
    Filename(&'a str),
}

pub fn open(target: OpenTarget, flags: c_int) -> Result<Handle> {
    let ptr = match target {
        OpenTarget::MainProgram => unsafe { dlopen(ptr::null_mut(), flags) },

        OpenTarget::Filename(filename) => {
            let filename = CString::new(filename)
                .chain_err(|| "unable to convert filename to a CString")?;
            unsafe { dlopen(filename.as_ptr(), flags) }
        }
    };

    if ptr.is_null() {
        let error = unsafe { CStr::from_ptr(dlerror()).to_string_lossy() };
        bail!("dlopen failed with `{}`", error);
    }

    Ok(Handle { ptr })
}

pub enum SymTarget {
    Default,
    Next,
}

pub fn sym(target: SymTarget, symbol: &str) -> Result<*mut c_void> {
    // Clear the previous error.
    unsafe {
        dlerror();
    }

    let target = match target {
        SymTarget::Default => RTLD_DEFAULT,
        SymTarget::Next => RTLD_NEXT,
    };

    let symbol = CString::new(symbol)
        .chain_err(|| "unable to convert symbol to a CString")?;

    let ptr = unsafe { dlsym(target, symbol.as_ptr()) };

    let error = unsafe { dlerror() };
    if !error.is_null() {
        let error = unsafe { CStr::from_ptr(error).to_string_lossy() };
        bail!("dlsym failed with `{}`", error);
    }

    Ok(ptr)
}
