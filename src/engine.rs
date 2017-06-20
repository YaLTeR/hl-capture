use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use errors::*;
use command;
use cvar::{CVar, cvar_t};
use hooks::hw;

static mut MAIN_THREAD_DATA: MainThreadDataContainer = MainThreadDataContainer {
    data: MainThreadData {
        capture_parameters: None,
        capture_sound: false,
        sound_remainder: 0f64,
        sound_capture_mode: ::hooks::hw::SoundCaptureMode::Normal,
        inside_key_event: false,
        time_interpolator: None,
    },
};

/// Global variables accessible from the main game thread.
pub struct MainThreadData {
    pub capture_parameters: Option<::capture::CaptureParameters>,
    pub capture_sound: bool,
    pub sound_remainder: f64,
    pub sound_capture_mode: ::hooks::hw::SoundCaptureMode,
    pub inside_key_event: bool,
    pub time_interpolator: Option<::capture::TimeInterpolator>,
}

/// A Send+Sync container to allow putting `MainThreadData` into a global variable.
struct MainThreadDataContainer {
    data: MainThreadData,
}

unsafe impl Send for MainThreadDataContainer {}
unsafe impl Sync for MainThreadDataContainer {}

/// A "container" for unsafe engine functions.
///
/// It's used for exposing safe interfaces for these functions where they can be used safely
/// (for example, in console command handlers). Engine also serves as a static guarantee of being
/// in the main game thread.
// Don't implement Clone/Copy, this will break EngineCVarGuard static guarantee.
pub struct Engine {
    /// This field serves two purposes:
    /// firstly, it prevents creating the struct not via the unsafe new() method,
    /// and secondly, it marks the struct as !Send and !Sync.
    _private: PhantomData<*const ()>,
}

/// A guard for statically ensuring that no engine functions are called
/// while the engine `CVar` reference is valid. Holds a mutable reference.
pub struct EngineCVarGuard<'a> {
    engine_cvar: &'a mut cvar_t,
    _borrow_guard: &'a mut Engine,
}

impl Engine {
    /// Creates an instance of Engine.
    ///
    /// # Safety
    /// Unsafe because this function should only be called from the main game thread.
    pub unsafe fn new() -> Self {
        Engine { _private: PhantomData }
    }

    /// Returns a mutable reference to the main thread global variables.
    pub fn data(&self) -> &mut MainThreadData {
        unsafe { &mut MAIN_THREAD_DATA.data }
    }

    /// Returns an iterator over the console command arguments.
    pub fn args(&self) -> command::Args {
        command::Args::new(self)
    }

    /// Prints the given string to the game console.
    pub fn con_print(&self, string: &str) {
        unsafe { hw::con_print(string) }
    }

    /// Returns the number of console command arguments.
    pub fn cmd_argc(&self) -> u32 {
        unsafe { hw::cmd_argc() }
    }

    /// Returns the console command argument with the given index.
    pub fn cmd_argv(&self, index: u32) -> String {
        unsafe { hw::cmd_argv(index) }
    }

    /// Registers the given console variable.
    pub fn register_variable(&mut self, cvar: &CVar) -> Result<()> {
        let mut engine_cvar = self.get_engine_cvar(cvar);

        ensure!(engine_cvar.string_is_non_null(),
                "attempted to register a variable with null string");

        unsafe {
            hw::register_variable(&mut engine_cvar);
        }

        Ok(())
    }

    /// Returns the engine CVar wrapped by the given CVar.
    ///
    /// Takes a mutable reference to Engine to statically ensure
    /// that no engine functions are called while the engine CVar reference is valid.
    pub fn get_engine_cvar(&mut self, cvar: &CVar) -> EngineCVarGuard {
        EngineCVarGuard {
            engine_cvar: unsafe { cvar.get_engine_cvar() },
            _borrow_guard: self,
        }
    }
}

impl<'a> Deref for EngineCVarGuard<'a> {
    type Target = cvar_t;

    fn deref(&self) -> &cvar_t {
        self.engine_cvar
    }
}

impl<'a> DerefMut for EngineCVarGuard<'a> {
    fn deref_mut(&mut self) -> &mut cvar_t {
        self.engine_cvar
    }
}
