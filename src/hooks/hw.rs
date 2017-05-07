#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use libc::*;

use dl;

lazy_static!{
    static ref ORIG_Host_Init: extern "C" fn(*mut c_void) -> c_int = {
        let ptr = dl::open(dl::OpenTarget::Filename("hw.so"), RTLD_NOW | RTLD_NOLOAD)
            .and_then(|h| h.sym("Host_Init"))
            .expect("error getting address of Host_Init");

        unsafe { *(&ptr as *const _ as *const _) }
    };

    static ref ORIG_Con_Printf: extern "C" fn(*const c_char) = {
        let ptr = dl::open(dl::OpenTarget::Filename("hw.so"), RTLD_NOW | RTLD_NOLOAD)
            .and_then(|h| h.sym("Con_Printf"))
            .expect("error getting address of Con_Printf");

        unsafe { *(&ptr as *const _ as *const _) }
    };
}

#[no_mangle]
pub extern "C" fn Host_Init(parms: *mut c_void) -> c_int {
    let rv = ORIG_Host_Init(parms);
    ORIG_Con_Printf(cstr!(b"Hello world!\0"));
    rv
}
