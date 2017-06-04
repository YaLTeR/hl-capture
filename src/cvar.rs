use libc::*;
use std::ffi::{CStr, CString};
use std::str::FromStr;

use engine::Engine;
use errors::*;

include!(concat!(env!("OUT_DIR"), "/cvar_array.rs"));

pub const EMPTY_CVAR_T: cvar_t = cvar_t {
    name: 0 as *const _,
    string: 0 as *mut _,
    flags: 0,
    value: 0f32,
    next: 0 as *mut _,
};

/// The engine CVar type.
#[repr(C)]
pub struct cvar_t {
    name: *const c_char,
    string: *mut c_char,
    flags: c_int,
    value: c_float,
    next: *mut cvar_t,
}

/// Safe wrapper for the engine CVar type.
pub struct CVar {
    /// This field has to be public because there's no const fn.
    /// It shouldn't be accessed manually.
    pub engine_cvar: *mut cvar_t, // This pointer is always valid and points to a 'static.

    /// This field has to be public because there's no const fn.
    /// It shouldn't be accessed manually.
    pub default_value: &'static str,

    /// This field has to be public because there's no const fn.
    /// It shouldn't be accessed manually.
    pub name: &'static str,
}

impl cvar_t {
    pub fn string_is_non_null(&self) -> bool {
        !self.string.is_null()
    }
}

impl CVar {
    /// Retrieves a mutable reference to the engine CVar.
    ///
    /// # Safety
    /// Unsafe because this function should only be called from the main game thread.
    /// You should also ensure that you don't call any engine functions while holding
    /// this reference, because the game also has a mutable reference to this CVar.
    pub unsafe fn get_engine_cvar(&self) -> &'static mut cvar_t {
        &mut *self.engine_cvar
    }

    /// Registers this console variable.
    pub fn register(&self, engine: &mut Engine) -> Result<()> {
        let ptr = {
            let mut engine_cvar = engine.get_engine_cvar(self);

            if engine_cvar.name.is_null() {
                let name_cstring = CString::new(self.name)
                    .chain_err(|| "could not convert the CVar name to CString")?;

                // HACK: leak this CString. It's staying around till the end anyway.
                engine_cvar.name = name_cstring.into_raw();
            }

            // This CString needs to be valid only for the duration of Cvar_RegisterVariable().
            let default_value_cstring = CString::new(self.default_value)
                .chain_err(|| "could not convert default CVar value to CString")?;

            let ptr = default_value_cstring.into_raw();
            engine_cvar.string = ptr;
            ptr
        };

        engine.register_variable(self).chain_err(|| "could not register the variable")?;

        // Free that CString from above.
        unsafe { CString::from_raw(ptr) };

        Ok(())
    }

    /// Returns the string this variable is set to.
    pub fn to_string(&self, engine: &mut Engine) -> Result<String> {
        let engine_cvar = engine.get_engine_cvar(self);
        ensure!(engine_cvar.string_is_non_null(), "the CVar string pointer was null");

        let string = unsafe { CStr::from_ptr(engine_cvar.string) }.to_str()
            .chain_err(|| "could not convert the CVar string to a Rust string")?;
        Ok(string.to_owned())
    }

    /// Tries parsing this variable's value to the desired type.
    pub fn parse<T>(&self, engine: &mut Engine) -> Result<T>
        where T: FromStr,
              <T as FromStr>::Err: ::std::error::Error + Send + 'static {
        let string = self.to_string(engine)
            .chain_err(|| "could not get this CVar's string value")?;
        string.parse()
            .chain_err(|| "could not convert the CVar string to the desired type")
    }
}
