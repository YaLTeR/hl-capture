#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use error_chain::ChainedError;
use libc::*;
use gl::types::*;
use std::cmp;
use std::ffi::{CStr, CString};
use std::sync::{Once, ONCE_INIT, RwLock};

use command;
use dl;
use encode;
use errors::*;
use function::Function;

lazy_static!{
    pub static ref POINTERS: RwLock<Pointers> = RwLock::new(Pointers::default());
}

#[derive(Debug, Default)]
pub struct Pointers {
    RunListenServer: Function<unsafe extern "C" fn(*mut c_void,
                                                   *mut c_char,
                                                   *mut c_char,
                                                   *mut c_char,
                                                   *mut c_void,
                                                   *mut c_void)
                                                   -> c_int>,

    Cmd_AddCommand: Function<unsafe extern "C" fn(*const c_char, *mut c_void)>,
    Cmd_Argc: Function<unsafe extern "C" fn() -> c_int>,
    Cmd_Argv: Function<unsafe extern "C" fn(c_int) -> *const c_char>,
    Con_Printf: Function<unsafe extern "C" fn(*const c_char)>,
    Memory_Init: Function<unsafe extern "C" fn(*mut c_void, c_int)>,
    GL_EndRendering: Function<unsafe extern "C" fn()>,

    s_BackBufferFBO: Option<*mut FBO_Container_t>,
}

// TODO: think about how to deal with unsafety here.
// The compiler complaining about *mut not being Send/Sync is perfectly valid here.
// Our safe RwLock cannot guarantee there isn't some game thread accessing the values
// at the same time. Although reading/writing to a pointer is already unsafe?
unsafe impl Send for Pointers {}
unsafe impl Sync for Pointers {}

#[repr(C)]
struct FBO_Container_t {
    s_hBackBufferFBO: GLuint,
    s_hBackBufferCB: GLuint,
    s_hBackBufferDB: GLuint,
    s_hBackBufferTex: GLuint,
}

/// The "main" function of hw.so, called inside `CEngineAPI::Run()`.
///
/// The game runs within this function and shortly after it exits hw.so is unloaded.
/// Note: `_restart` also causes this function to exit, in this case the launcher
/// unloads and reloads hw.so and this function is called again as if it was a fresh start.
#[export_name = "_Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_"]
pub unsafe extern "C" fn RunListenServer(instance: *mut c_void,
                                         basedir: *mut c_char,
                                         cmdline: *mut c_char,
                                         postRestartCmdLineArgs: *mut c_char,
                                         launcherFactory: *mut c_void,
                                         filesystemFactory: *mut c_void)
                                         -> c_int {
    // hw.so just loaded, either for the first time or potentially at a different address.
    // Refresh all pointers.
    if let Err(ref e) = refresh_pointers().chain_err(|| "error refreshing pointers") {
        panic!("{}", e.display());
    }

    // Initialize the encoding.
    {
        static INIT: Once = ONCE_INIT;
        INIT.call_once(|| if let Err(ref e) = encode::initialize()
                              .chain_err(|| "error initializing encoding") {
                           panic!("{}", e.display());
                       });
    }

    let rv = real!(RunListenServer)(instance,
                                    basedir,
                                    cmdline,
                                    postRestartCmdLineArgs,
                                    launcherFactory,
                                    filesystemFactory);

    // Since hw.so is getting unloaded, reset all pointers.
    reset_pointers();

    rv
}

/// Initializes the hunk memory.
///
/// After the hunk memory has been initialized we can register console commands and variables.
#[no_mangle]
pub unsafe extern "C" fn Memory_Init(buf: *mut c_void, size: c_int) {
    real!(Memory_Init)(buf, size);

    register_cvars_and_commands();
}

/// Blits pixels from the framebuffer to screen and flips.
///
/// If framebuffers aren't used, simply flips the screen.
#[no_mangle]
pub unsafe extern "C" fn GL_EndRendering() {
    real!(GL_EndRendering)();

    // TODO: check if we're called from SCR_UpdateScreen().
}

/// Obtains and stores all necessary function and variable addresses.
fn refresh_pointers() -> Result<()> {
    let hw = dl::open("hw.so", RTLD_NOW | RTLD_NOLOAD)
        .chain_err(|| "couldn't load hw.so")?;

    let mut pointers = POINTERS.write().unwrap();

    unsafe {
        find!(pointers,
              hw,
              RunListenServer,
              "_Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_");
        find!(pointers, hw, Cmd_AddCommand, "Cmd_AddCommand");
        find!(pointers, hw, Cmd_Argc, "Cmd_Argc");
        find!(pointers, hw, Cmd_Argv, "Cmd_Argv");
        find!(pointers, hw, Con_Printf, "Con_Printf");
        find!(pointers, hw, Memory_Init, "Memory_Init");
        find!(pointers, hw, GL_EndRendering, "GL_EndRendering");

        pointers.s_BackBufferFBO = Some(hw.sym("s_BackBufferFBO")
                .chain_err(|| "couldn't find s_BackBufferFBO")? as _);
    }

    Ok(())
}

/// Resets all pointers to their default values.
fn reset_pointers() {
    *POINTERS.write().unwrap() = Pointers::default();
}

/// Registers console commands and variables.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
unsafe fn register_cvars_and_commands() {
    for cmd in command::COMMANDS.read().unwrap().iter() {
        register_command(cmd.name(), cmd.callback());
    }
}

/// Registers a console command.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
unsafe fn register_command(name: &'static [u8], callback: unsafe extern "C" fn()) {
    real!(Cmd_AddCommand)(name as *const _ as *const _, callback as *mut c_void);
}

/// Prints the given string to the game console.
///
/// `string` must not contain null bytes.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
pub unsafe fn con_print(string: &str) {
    real!(Con_Printf)(CString::new(string.replace('%', "%%"))
                          .expect("string cannot contain null bytes")
                          .as_ptr())
}

/// Gets the console command argument count.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
pub unsafe fn cmd_argc() -> u32 {
    let argc = real!(Cmd_Argc)();
    cmp::max(0, argc) as u32
}

/// Gets a console command argument.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
pub unsafe fn cmd_argv(index: u32) -> String {
    let index = cmp::min(index, i32::max_value() as u32) as i32;
    let arg = real!(Cmd_Argv)(index);
    CStr::from_ptr(arg).to_string_lossy().into_owned()
}

command!(cap_test, |engine| {
    let args = engine.args();

    let mut buf = String::new();
    buf.push_str(&format!("Args len: {}\n", args.len()));

    for arg in args {
        buf.push_str(&arg);
        buf.push('\n');
    }

    engine.con_print(&buf);
});

command!(cap_another_test, |engine| {
    engine.con_print("Hello! %s %d %\n");
});
