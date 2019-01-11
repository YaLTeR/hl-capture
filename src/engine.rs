use failure::Error;
use ocl;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::result;

use command;
use cvar::{cvar_t, CVar};
use hooks::hw;
use utils::MaybeUnavailable;

type Result<T> = result::Result<T, Error>;

static mut MAIN_THREAD_DATA: MainThreadDataContainer =
    MainThreadDataContainer { data: MainThreadData { capture_parameters: None,
                                                     capture_sound: false,
                                                     sound_remainder: 0f64,
                                                     sound_capture_mode:
                                                         ::hooks::hw::SoundCaptureMode::Normal,
                                                     inside_key_event: false,
                                                     inside_gl_setmode: false,
                                                     fps_converter: None,
                                                     encoder_pixel_format: None,
                                                     pro_que: MaybeUnavailable::NotChecked,
                                                     ocl_yuv_buffers:
                                                         MaybeUnavailable::NotChecked } };

/// Global variables accessible from the main game thread.
pub struct MainThreadData {
    pub capture_parameters: Option<::capture::CaptureParameters>,
    pub capture_sound: bool,
    pub sound_remainder: f64,
    pub sound_capture_mode: ::hooks::hw::SoundCaptureMode,
    pub inside_key_event: bool,
    pub inside_gl_setmode: bool,
    pub fps_converter: Option<::fps_converter::FPSConverters>,
    pub encoder_pixel_format: Option<::ffmpeg::format::Pixel>,
    pub pro_que: MaybeUnavailable<ocl::ProQue>,
    pub ocl_yuv_buffers: MaybeUnavailable<(ocl::Buffer<u8>, ocl::Buffer<u8>, ocl::Buffer<u8>)>,
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

/// A static guarantee of being in the main game thread, separate from `Engine` to allow borrowing.
#[derive(Clone, Copy)]
pub struct MainThreadMarker<'engine> {
    // Same purpose as in Engine.
    _private: PhantomData<*const ()>,
    _private_2: PhantomData<&'engine ()>,
}

impl Engine {
    /// Creates an instance of Engine.
    ///
    /// # Safety
    /// Unsafe because this function should only be called from the main game thread.
    #[inline]
    pub unsafe fn new() -> Self {
        Engine { _private: PhantomData }
    }

    /// Splits off a `MainThreadMarker`.
    #[inline]
    pub fn marker(&self) -> (&Self, MainThreadMarker) {
        (self, unsafe { MainThreadMarker::new() })
    }

    /// Splits off a `MainThreadMarker` from a mutable reference.
    #[inline]
    pub fn marker_mut(&mut self) -> (&mut Self, MainThreadMarker) {
        (self, unsafe { MainThreadMarker::new() })
    }

    /// Returns a reference to the main thread global variables.
    #[inline]
    pub fn data(&self) -> &MainThreadData {
        unsafe { &MAIN_THREAD_DATA.data }
    }

    /// Returns a mutable reference to the main thread global variables.
    #[inline]
    pub fn data_mut(&mut self) -> &mut MainThreadData {
        unsafe { &mut MAIN_THREAD_DATA.data }
    }

    /// Returns an iterator over the console command arguments.
    #[inline]
    pub fn args(&self) -> command::Args {
        command::Args::new(self)
    }

    /// Prints the given string to the game console.
    #[inline]
    pub fn con_print(&self, string: &str) {
        unsafe { hw::con_print(string) }
    }

    /// Returns the number of console command arguments.
    #[inline]
    pub fn cmd_argc(&self) -> u32 {
        unsafe { hw::cmd_argc() }
    }

    /// Returns the console command argument with the given index.
    #[inline]
    pub fn cmd_argv(&self, index: u32) -> String {
        unsafe { hw::cmd_argv(index) }
    }

    /// Registers the given console variable.
    #[inline]
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
    #[inline]
    pub fn get_engine_cvar(&mut self, cvar: &CVar) -> EngineCVarGuard {
        EngineCVarGuard { engine_cvar: unsafe { cvar.get_engine_cvar() },
                          _borrow_guard: self }
    }
}

impl<'a> Deref for EngineCVarGuard<'a> {
    type Target = cvar_t;

    #[inline]
    fn deref(&self) -> &cvar_t {
        self.engine_cvar
    }
}

impl<'a> DerefMut for EngineCVarGuard<'a> {
    #[inline]
    fn deref_mut(&mut self) -> &mut cvar_t {
        self.engine_cvar
    }
}

impl<'engine> MainThreadMarker<'engine> {
    #[inline]
    unsafe fn new() -> Self {
        Self { _private: PhantomData,
               _private_2: PhantomData }
    }
}
