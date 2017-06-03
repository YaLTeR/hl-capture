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

#[repr(C)]
pub struct cvar_t {
    pub name: *const c_char,
    pub string: *mut c_char,
    flags: c_int,
    value: c_float,
    next: *mut cvar_t,
}

pub struct CVar {
    engine_cvar: *mut cvar_t,
    default_value: &'static str,
    name: &'static str,
}

impl CVar {
    /// Creates a new CVar instance.
    pub fn new(engine_cvar: &mut cvar_t, name: &'static str, default_value: &'static str) -> Self {
        Self {
            engine_cvar,
            default_value,
            name,
        }
    }

    /// Retrieves a reference to the engine CVar.
    ///
    /// # Safety
    /// Unsafe because this function should only be called from the main game thread.
    /// You should also ensure that you don't call any engine functions while holding
    /// this reference, because the game also has a mutable reference to this CVar.
    pub unsafe fn get_engine_cvar(&self) -> Result<&cvar_t> {
        ensure!(!self.engine_cvar.is_null(), "engine_cvar is null");

        Ok(&*self.engine_cvar)
    }

    /// Retrieves a mutable reference to the engine CVar.
    ///
    /// # Safety
    /// Unsafe because this function should only be called from the main game thread.
    /// You should also ensure that you don't call any engine functions while holding
    /// this reference, because the game also has a mutable reference to this CVar.
    pub unsafe fn get_engine_cvar_mut(&self) -> Result<&mut cvar_t> {
        ensure!(!self.engine_cvar.is_null(), "engine_cvar is null");

        Ok(&mut *self.engine_cvar)
    }

    /// Registers this console variable.
    pub fn register(&self, engine: &Engine) -> Result<()> {
        let ptr = {
            let engine_cvar = unsafe { self.get_engine_cvar_mut()? };

            if engine_cvar.name.is_null() {
                let name_cstring = CString::new(self.name)
                    .chain_err(|| "could not convert CVar name to CString")?;

                // HACK: leak name_cstring.
                engine_cvar.name = name_cstring.into_raw();
            }

            // This CString needs to be valid only for the duration of Cvar_RegisterVariable().
            let default_value_cstring = CString::new(self.default_value)
                .chain_err(|| "could not convert CVar name to CString")?;

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
    ///
    /// # Safety
    /// Unsafe because this function should only be called from the main game thread.
    pub unsafe fn to_string(&self) -> Result<String> {
        let engine_cvar = self.get_engine_cvar()?;
        ensure!(!engine_cvar.string.is_null(), "the CVar string pointer was null");

        Ok(CStr::from_ptr(engine_cvar.string).to_string_lossy().into_owned())
    }

    /// Tries parsing this variable's value to the desired type.
    ///
    /// # Safety
    /// Unsafe because this function should only be called from the main game thread.
    pub unsafe fn parse<T>(&self) -> Result<T>
        where T: FromStr,
              <T as FromStr>::Err: ::std::error::Error + Send + 'static {
        let string = self.to_string()
            .chain_err(|| "could not get this CVar's string value")?;
        string.parse()
            .chain_err(|| "could not convert the CVar string to the desired type")
    }
}
