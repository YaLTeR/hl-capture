/// Returns the original function pointer.
///
/// This should only be used from the main game thread.
macro_rules! real {
    ($f:ident) => (FUNCTIONS.as_ref().unwrap().$f)
}

/// Returns a pointer from the POINTERS variable.
///
/// This should only be used from the main game thread.
macro_rules! ptr {
    ($f:ident) => (POINTERS.as_ref().unwrap().$f)
}

macro_rules! find {
    ($handle:expr, $symbol:tt) => ({
        *(&$handle.sym($symbol)
                  .chain_err(|| concat!("couldn't find ", $symbol))? as *const _ as *const _)
    })
}

/// Defines console commands.
///
/// Commands defined by this macro will be automatically added
/// to the console command list and registered in the game.
macro_rules! command {
    ($name:ident, $callback:expr) => (
        #[allow(non_camel_case_types)]
        pub struct $name;

        impl $name {
            // This will get called by the engine, in main game thread.
            unsafe extern "C" fn callback() {
                const F: &Fn(::engine::Engine) = &$callback;

                // We know this is the main game thread.
                let engine = ::engine::Engine::new();

                F(engine);
            }
        }

        impl ::command::Command for $name {
            fn name(&self) -> &'static [u8] {
                lazy_static! {
                    static ref NAME: ::std::ffi::CString = {
                        ::std::ffi::CString::new(stringify!($name)).unwrap()
                    };
                }

                NAME.as_bytes_with_nul()
            }

            fn callback(&self) -> unsafe extern "C" fn() {
                Self::callback
            }
        }
    )
}

/// Defines console variables.
///
/// Variables defined by this macro will be automatically added
/// to the console command list and registered in the game.
macro_rules! cvar {
    ($name:ident, $default_value:expr) => (
        #[allow(non_upper_case_globals)]
        pub static $name: ::cvar::CVar = ::cvar::CVar {
            engine_cvar: {
                static mut ENGINE_CVAR: ::cvar::cvar_t = ::cvar::EMPTY_CVAR_T;
                unsafe { &ENGINE_CVAR as *const _ as *mut _ }
            },
            default_value: $default_value,
            name: stringify!($name),
        };
    )
}
