use libc::*;
use std::ffi::{CStr, CString};
use std::str::FromStr;

use errors::*;
use hooks::hw;

include!(concat!(env!("OUT_DIR"), "/cvar_array.rs"));

pub const EMPTY_CVAR: CVar = CVar {
    engine_cvar: cvar_t {
        name: 0 as *const _,
        string: 0 as *mut _,
        flags: 0,
        value: 0f32,
        next: 0 as *mut _,
    },
    default_value: "",
    name: "",
};

#[repr(C)]
pub struct cvar_t {
    name: *const c_char,
    string: *mut c_char,
    flags: c_int,
    value: c_float,
    next: *mut cvar_t,
}

// TODO: figure out the correct way of dealing with unsafety.
// The problem here is that when we access cvars they could be
// modified by the game thread at the same time.
unsafe impl Sync for cvar_t {}

// Fields are public for the cvar! macro. const fn would really help here.
pub struct CVar {
    pub engine_cvar: cvar_t,
    pub default_value: &'static str,
    pub name: &'static str,
}

impl CVar {
    /// Registers this console variable.
    ///
    /// # Safety
    /// Unsafe because this function should only be called from the main game thread.
    pub unsafe fn register(&mut self) -> Result<()> {
        if self.engine_cvar.name.is_null() {
            let name_cstring = CString::new(self.name)
                .chain_err(|| "could not convert CVar name to CString")?;

            // HACK: leak name_cstring. I don't see a better way of handling this currently,
            // I cannot store this CString inside CVar (because I can't initialize it without
            // calling CString::new()), and destructors aren't run for static muts anyway.
            self.engine_cvar.name = name_cstring.into_raw();
        }

        // This CString needs to be valid only for the duration of Cvar_RegisterVariable().
        let default_value_cstring = CString::new(self.default_value)
            .chain_err(|| "could not convert CVar name to CString")?;

        let ptr = default_value_cstring.into_raw();
        self.engine_cvar.string = ptr;

        hw::register_variable(&mut self.engine_cvar);

        // Free that CString from above.
        CString::from_raw(ptr);

        Ok(())
    }

    /// Tries parsing this variable's value to the desired type.
    pub fn parse<T>(&self) -> Result<T>
        where T: FromStr,
              <T as FromStr>::Err: ::std::error::Error + Send + 'static {
        ensure!(!self.engine_cvar.string.is_null(), "the CVar string pointer was null");

        let string = unsafe { CStr::from_ptr(self.engine_cvar.string) }.to_string_lossy();
        string.parse().chain_err(|| "could not convert the CVar string to the desired type")
    }
}
