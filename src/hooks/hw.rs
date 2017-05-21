#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use error_chain::ChainedError;
use libc::*;
use gl;
use gl::types::*;
use std::cmp;
use std::ffi::{CStr, CString};
use std::mem;
use std::ptr;
use std::sync::{Once, ONCE_INIT, RwLock};

use command;
use dl;
use encode;
use errors::*;
use function::Function;
use sdl;

lazy_static!{
    static ref POINTERS: RwLock<Pointers> = RwLock::new(Pointers::default());
    static ref CAPTURING: RwLock<bool> = RwLock::new(false);
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
    Sys_VID_FlipScreen: Function<unsafe extern "C" fn()>,
    VideoMode_GetCurrentVideoMode: Function<unsafe extern "C" fn(*mut c_int,
                                                                 *mut c_int,
                                                                 *mut c_int)>,
    VideoMode_IsWindowed: Function<unsafe extern "C" fn() -> c_int>,

    window_rect: Option<*mut RECT>,
    s_BackBufferFBO: Option<*mut FBO_Container_t>,
}

// TODO: think about how to deal with unsafety here.
// The compiler complaining about *mut not being Send/Sync is perfectly valid here.
// Our safe RwLock cannot guarantee there isn't some game thread accessing the values
// at the same time. Although reading/writing to a pointer is already unsafe?
unsafe impl Send for Pointers {}
unsafe impl Sync for Pointers {}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct RECT {
    left: c_int,
    right: c_int,
    top: c_int,
    bottom: c_int,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
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
/// It's also a good place to get OpenGL function pointers.
#[no_mangle]
pub unsafe extern "C" fn Memory_Init(buf: *mut c_void, size: c_int) {
    real!(Memory_Init)(buf, size);

    register_cvars_and_commands();

    gl::ReadPixels::load_with(|s| sdl::get_proc_address(s) as _);
    if !gl::ReadPixels::is_loaded() {
        panic!("could not load glReadPixels()");
    }
}

/// Blits pixels from the framebuffer to screen and flips.
///
/// If framebuffers aren't used, simply flips the screen.
#[export_name = "_Z18Sys_VID_FlipScreenv"]
pub unsafe extern "C" fn Sys_VID_FlipScreen() {
    let (w, h) = get_resolution();
    let buf = encode::get_buffer((w, h));

    if *CAPTURING.read().unwrap() {
        gl::ReadPixels(0, 0, w as GLsizei, h as GLsizei,
                       gl::RGB, gl::UNSIGNED_BYTE,
                       buf.as_mut_ptr() as _);
    }

    real!(Sys_VID_FlipScreen)();

    // TODO: check if we're called from SCR_UpdateScreen().

    // println!("s_BackBufferFBO: {:?}", *POINTERS.read().unwrap().s_BackBufferFBO.unwrap());
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
        find!(pointers, hw, Sys_VID_FlipScreen, "_Z18Sys_VID_FlipScreenv");
        find!(pointers, hw, VideoMode_GetCurrentVideoMode, "VideoMode_GetCurrentVideoMode");
        find!(pointers, hw, VideoMode_IsWindowed, "VideoMode_IsWindowed");

        pointers.window_rect = Some(hw.sym("window_rect")
                .chain_err(|| "couldn't find window_rect")? as _);
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

/// Returns the current game resolution.
///
/// # Safety
/// Unsafe because this function should only be called from the main game thread.
pub unsafe fn get_resolution() -> (u32, u32) {
    let mut width;
    let mut height;

    if real!(VideoMode_IsWindowed)() != 0 {
        let window_rect = *POINTERS.read().unwrap().window_rect.unwrap();
        width = window_rect.right - window_rect.left;
        height = window_rect.bottom - window_rect.top;
    } else {
        width = mem::uninitialized();
        height = mem::uninitialized();
        real!(VideoMode_GetCurrentVideoMode)(&mut width, &mut height, ptr::null_mut());
    }

    width = cmp::max(0, width);
    height = cmp::max(0, height);

    (width as u32, height as u32)
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

command!(cap_start, |_engine| {
    *CAPTURING.write().unwrap() = true;
});

command!(cap_stop, |_engine| {
    *CAPTURING.write().unwrap() = false;
});
