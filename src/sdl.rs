use libc::*;
use sdl2_sys;
use std::ffi::CString;

pub fn get_proc_address(name: &str) -> *const c_void {
    unsafe {
        sdl2_sys::SDL_GL_GetProcAddress(CString::new(name)
                                            .expect("could not convert name to a CString")
                                            .as_ptr())
    }
}
