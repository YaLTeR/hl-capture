#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use error_chain::ChainedError;
use libc::*;
use std::sync::RwLock;

use command;
use dl;
use errors::*;
use function::Function;

lazy_static!{
    static ref POINTERS: RwLock<Pointers> = RwLock::new(Pointers::default());
}

#[derive(Debug, Default)]
struct Pointers {
    RunListenServer: Function<extern "C" fn(*mut c_void,
                                            *mut c_char,
                                            *mut c_char,
                                            *mut c_char,
                                            *mut c_void,
                                            *mut c_void) -> c_int>,

    Cmd_AddCommand: Function<extern "C" fn(*const c_char, *mut c_void)>,
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
                                  filesystemFactory: *mut c_void) -> c_int {
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
    println!("refresh_pointers()");

    let hw = dl::open("hw.so", RTLD_NOW | RTLD_NOLOAD).chain_err(|| "couldn't load hw.so")?;

    let mut pointers = POINTERS.write().unwrap();

    unsafe {
        find!(pointers, hw, RunListenServer, "_Z15RunListenServerPvPcS0_S0_PFP14IBaseInterfacePKcPiES7_");
        find!(pointers, hw, Cmd_AddCommand, "Cmd_AddCommand");
        find!(pointers, hw, Con_Printf, "Con_Printf");
        find!(pointers, hw, Memory_Init, "Memory_Init");
    }

    Ok(())
}

/// Reset all pointers to their default values.
fn reset_pointers() {
    println!("reset_pointers()");

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

command!(cap_test, b"cap_test\0", |args| {
    println!("len: {}", args().len());
});

command!(cap_another_test, b"cap_another_test\0", |_args| {
    real!(Con_Printf)(cstr!(b"Hello!\0"));
});
