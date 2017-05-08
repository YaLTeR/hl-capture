#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use error_chain::ChainedError;
use libc::*;
use std::ffi::CString;
use std::sync::RwLock;

use command;
use dl;
use errors::*;
use function::Function;

lazy_static!{
    pub static ref POINTERS: RwLock<Pointers> = RwLock::new(Pointers::default());
}

#[derive(Debug, Default)]
pub struct Pointers {
    RunListenServer: Function<extern "C" fn(*mut c_void,
                                            *mut c_char,
                                            *mut c_char,
                                            *mut c_char,
                                            *mut c_void,
                                            *mut c_void)
                                            -> c_int>,

    Cmd_AddCommand: Function<extern "C" fn(*const c_char, *mut c_void)>,
    pub Cmd_Argc: Function<extern "C" fn() -> c_int>,
    pub Cmd_Argv: Function<extern "C" fn(c_int) -> *const c_char>,
    Con_Printf: Function<extern "C" fn(*const c_char)>,
    Memory_Init: Function<extern "C" fn(*mut c_void, c_int)>,
}

/// This is the "main" function of hw.so, called inside CEngineAPI::Run().
/// The game runs within this function and shortly after it exits hw.so is unloaded.
/// Note: _restart also causes this function to exit, in this case the launcher
/// unloads and reloads hw.so and this function is called again as if it was a fresh start.
#[export_name = "_Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_"]
pub extern "C" fn RunListenServer(instance: *mut c_void,
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

/// This function initializes the hunk memory.
/// After the hunk memory is initialized we can register console commands and variables.
#[no_mangle]
pub extern "C" fn Memory_Init(buf: *mut c_void, size: c_int) {
    let rv = real!(Memory_Init)(buf, size);

    register_cvars_and_commands();

    rv
}

/// Open hw.so, then get and store all necessary function and variable addresses.
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
    }

    Ok(())
}

/// Reset all pointers to their default values.
fn reset_pointers() {
    *POINTERS.write().unwrap() = Pointers::default();
}

/// Register console commands and variables.
fn register_cvars_and_commands() {
    for cmd in command::COMMANDS.read().unwrap().iter() {
        register_command(cmd.name(), cmd.callback());
    }
}

fn register_command(name: &'static [u8], callback: extern "C" fn()) {
    real!(Cmd_AddCommand)(name as *const _ as *const _, callback as *mut c_void);
}

fn con_printf(string: &str) {
    real!(Con_Printf)(CString::new(string.replace('%', "%%"))
                          .expect("string cannot contain null bytes")
                          .as_ptr());
}

command!(cap_test, b"cap_test\0", |args| {
    let a = args();

    let mut buf = String::new();
    buf.push_str(&format!("Args len: {}\n", a.len()));

    for arg in a {
        buf.push_str(&arg);
        buf.push('\n');
    }

    con_printf(&buf);
});

command!(cap_another_test, b"cap_another_test\0", |_args| {
    con_printf("Hello! %s %d %\n");
});
