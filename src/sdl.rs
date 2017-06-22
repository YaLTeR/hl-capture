use libc::*;
use sdl2_sys;
use std::ffi::CString;

pub fn get_proc_address(name: &str) -> *const c_void {
    let cstring = CString::new(name).expect("could not convert name to a CString");
    unsafe { sdl2_sys::SDL_GL_GetProcAddress(cstring.as_ptr()) }
}

pub fn get_current_context() -> u32 {
    unsafe { sdl2_sys::SDL_GL_GetCurrentContext() as u32 }
}
