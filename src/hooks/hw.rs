#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use libc::*;

use dl;

lazy_static!{
    static ref ORIG_RunListenServer: extern "C" fn(*mut c_void,
                                                   *mut c_char,
                                                   *mut c_char,
                                                   *mut c_char,
                                                   *mut c_void,
                                                   *mut c_void) -> c_int = {
        let ptr = dl::open("hw.so", RTLD_NOW | RTLD_NOLOAD)
            .and_then(|h| h.sym("_Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_"))
            .expect("error getting address of RunListenServer()");

        unsafe { *(&ptr as *const _ as *const _) }
    };

    static ref ORIG_Host_Init: extern "C" fn(*mut c_void) -> c_int = {
        let ptr = dl::open("hw.so", RTLD_NOW | RTLD_NOLOAD)
            .and_then(|h| h.sym("Host_Init"))
            .expect("error getting address of Host_Init()");

        unsafe { *(&ptr as *const _ as *const _) }
    };

    static ref ORIG_Con_Printf: extern "C" fn(*const c_char) = {
        let ptr = dl::open("hw.so", RTLD_NOW | RTLD_NOLOAD)
            .and_then(|h| h.sym("Con_Printf"))
            .expect("error getting address of Con_Printf()");

        unsafe { *(&ptr as *const _ as *const _) }
    };
}

/// This is the "main" function of hw.so, called inside CEngineAPI::Run.
/// The game runs within this function and shortly after it exits hw.so is unloaded.
/// Note: _restart also causes this function to exit, in this case the launcher
/// unloads and reloads hw.so and this function is called again as if it was a fresh start.
#[no_mangle]
pub extern "C" fn _Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_(instance: *mut c_void,
                                                                            basedir: *mut c_char,
                                                                            cmdline: *mut c_char,
                                                                            postRestartCmdLineArgs: *mut c_char,
                                                                            launcherFactory: *mut c_void,
                                                                            filesystemFactory: *mut c_void) -> c_int {
    refresh_pointers();

    let rv = ORIG_RunListenServer(instance,
                                  basedir,
                                  cmdline,
                                  postRestartCmdLineArgs,
                                  launcherFactory,
                                  filesystemFactory);

    rv
}

#[no_mangle]
pub extern "C" fn Host_Init(parms: *mut c_void) -> c_int {
    let rv = ORIG_Host_Init(parms);
    ORIG_Con_Printf(cstr!(b"Hello world!\0"));
    rv
}

/// Open hw.so, then get and store all necessary function and variable addresses.
fn refresh_pointers() {
    println!("refresh_pointers()");
}
