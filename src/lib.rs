#![allow(non_snake_case)]

extern crate libc;

use libc::*;

#[macro_use]
mod macros;

#[no_mangle]
pub extern "C" fn Host_Init(parms: *mut c_void) -> c_int {
    unsafe {
        let hw = dlopen(cstr!(b"hw.so\0"), RTLD_NOW | RTLD_NOLOAD);
        if hw.is_null() { panic!("error opening hw.so"); }

        let con_printf_ptr = dlsym(hw, cstr!(b"Con_Printf\0"));
        if con_printf_ptr.is_null() { panic!("error getting Con_Printf"); }

        let Con_Printf = *(&con_printf_ptr as *const _ as *const extern "C" fn(*const c_char));
        Con_Printf(cstr!(b"Hello world!\0"));

        let host_init_ptr = dlsym(hw, cstr!(b"Host_Init\0"));
        if host_init_ptr.is_null() { panic!("error getting Host_Init"); }

        let Host_Init = *(&host_init_ptr as *const _ as *const extern "C" fn(*mut c_void) -> c_int);
        Host_Init(parms)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
    }
}
