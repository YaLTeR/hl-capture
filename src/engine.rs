use std::marker::PhantomData;
use std::str::FromStr;

use errors::*;
use command;
use cvar::CVar;
use hooks::hw;

/// A "container" for unsafe engine functions.
///
/// It's used for exposing safe interfaces for these functions where they can be used safely
/// (for example, in console command handlers).
pub struct Engine {
    /// This field serves two purposes:
    /// firstly, it prevents creating the struct not via the unsafe new() method,
    /// and secondly, it marks the struct as !Send and !Sync.
    _private: PhantomData<*const ()>,
}

impl Engine {
    /// Creates an instance of Engine.
    ///
    /// # Safety
    /// Unsafe because this function should only be called from the main game thread.
    pub unsafe fn new() -> Self {
        Engine { _private: PhantomData }
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
    pub fn register_variable(&self, cvar: &CVar) -> Result<()> {
        let mut engine_cvar = unsafe { cvar.get_engine_cvar_mut()? };
        ensure!(!engine_cvar.name.is_null(), "attempted to register a variable with null name");
        ensure!(!engine_cvar.string.is_null(), "attempted to register a variable with null string");

        unsafe { hw::register_variable(&mut engine_cvar); }

        Ok(())
    }

    /// Returns the string this variable is set to.
    pub fn cvar_to_string(&self, cvar: &CVar) -> Result<String> {
        unsafe { cvar.to_string() }
    }

    /// Tries parsing this variable's value to the desired type.
    pub fn cvar_parse<T>(&self, cvar: &CVar) -> Result<T>
        where T: FromStr,
              <T as FromStr>::Err: ::std::error::Error + Send + 'static {
        unsafe { cvar.parse() }
    }
}
