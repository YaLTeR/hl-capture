use std::marker::PhantomData;

use command;
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
}
