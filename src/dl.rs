use libc::*;
use std::ffi::{CStr, CString};

use errors::*;

/// A container for a `dlopen()` handle.
pub struct Handle {
    /// The handle returned by `dlopen()`.
    ptr: *mut c_void,
}

unsafe impl Sync for Handle {}

impl Drop for Handle {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            dlclose(self.ptr);
        }
    }
}

impl Handle {
    /// Obtains a symbol address using `dlsym()`.
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

/// Opens a dynamic library and returns the resulting handle.
pub fn open(filename: &str, flags: c_int) -> Result<Handle> {
    let filename = CString::new(filename)
        .chain_err(|| "unable to convert filename to a CString")?;

    let ptr = unsafe { dlopen(filename.as_ptr(), flags) };

    if ptr.is_null() {
        let error = unsafe { CStr::from_ptr(dlerror()).to_string_lossy() };
        bail!("dlopen failed with `{}`", error);
    }

    Ok(Handle { ptr })
}
