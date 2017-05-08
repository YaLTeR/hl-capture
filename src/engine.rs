use std::marker::PhantomData;

use command;
use hooks::hw;

/// This struct is a "container" for the engine functions, which are not thread-safe.
/// It's used for exposing safe interfaces for these functions where they can be used
/// (for example, in console command handlers).
pub struct Engine {
    /// This field serves two purposes:
    /// firstly, it prevents creating the struct not via the unsafe new() method,
    /// as secondly, it marks the struct as !Send and !Sync.
    _private: PhantomData<*const ()>,
}

impl Engine {
    /// Unsafe because it should only be called from the main game thread.
    pub unsafe fn new() -> Self {
        Engine { _private: PhantomData }
    }

    pub fn args<'a>(&'a self) -> command::Args<'a> {
        command::Args::new(&self)
    }

    #[inline(always)]
    pub fn con_print(&self, string: &str) {
        unsafe { hw::con_print(string) }
    }

    #[inline(always)]
    pub fn cmd_argc(&self) -> u32 {
        unsafe { hw::cmd_argc() }
    }

    #[inline(always)]
    pub fn cmd_argv(&self, index: u32) -> String {
        unsafe { hw::cmd_argv(index) }
    }
}
