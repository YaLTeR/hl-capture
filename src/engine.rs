use failure::{bail, ensure, Error};
use ocl;
use std::cell::{Ref, RefMut};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::result;

use crate::command;
use crate::cvar::{cvar_t, CVar};
use crate::hooks::hw;
use crate::utils::{MaybeUnavailable, RacyRefCell};

type Result<T> = result::Result<T, Error>;

/// The `Engine` instance.
static ENGINE: RacyRefCell<Engine> = RacyRefCell::new(unsafe { Engine::new() });
/// The `MainThreadGlobals` instance.
static GLOBALS: RacyRefCell<MainThreadGlobals> = RacyRefCell::new(MainThreadGlobals::new());

/// Global variables accessible from the main game thread.
pub struct MainThreadGlobals {
    pub capture_parameters: Option<crate::capture::CaptureParameters>,
    pub capture_sound: bool,
    pub sound_remainder: f64,
    pub sound_capture_mode: crate::hooks::hw::SoundCaptureMode,
    pub inside_key_event: bool,
    pub inside_gl_setmode: bool,
    pub fps_converter: Option<crate::fps_converter::FPSConverters>,
    pub encoder_pixel_format: Option<::ffmpeg::format::Pixel>,
    pub pro_que: MaybeUnavailable<ocl::ProQue>,
    pub ocl_yuv_buffers: MaybeUnavailable<(ocl::Buffer<u8>, ocl::Buffer<u8>, ocl::Buffer<u8>)>,
}

impl MainThreadGlobals {
    /// Returns new `MainThreadGlobals`.
    #[inline]
    const fn new() -> Self {
        Self { capture_parameters: None,
               capture_sound: false,
               sound_remainder: 0f64,
               sound_capture_mode: crate::hooks::hw::SoundCaptureMode::Normal,
               inside_key_event: false,
               inside_gl_setmode: false,
               fps_converter: None,
               encoder_pixel_format: None,
               pro_que: MaybeUnavailable::NotChecked,
               ocl_yuv_buffers: MaybeUnavailable::NotChecked }
    }
}

/// This marker serves as a static guarantee of being on the main game thread. Functions that
/// should only be called from the main game thread should accept an argument of this type.
#[derive(Clone, Copy)]
pub struct MainThreadMarker {
    // Mark as !Send and !Sync.
    _marker: PhantomData<*const ()>,
}

impl MainThreadMarker {
    /// Creates a new `MainThreadMarker`.
    ///
    /// # Safety
    /// This should only be called from the main game thread.
    #[inline]
    pub unsafe fn new() -> Self {
        Self { _marker: PhantomData }
    }

    /// Returns an immutable reference to `Engine`.
    #[inline]
    pub fn engine(self) -> Ref<'static, Engine> {
        // We know we're on the main thread because we accept self which is a MainThreadMarker.
        unsafe { ENGINE.borrow() }
    }

    /// Returns a mutable reference to `Engine`.
    #[inline]
    pub fn engine_mut(self) -> RefMut<'static, Engine> {
        // We know we're on the main thread because we accept self which is a MainThreadMarker.
        unsafe { ENGINE.borrow_mut() }
    }

    /// Returns an immutable reference to `MainThreadGlobals`.
    #[inline]
    pub fn globals(self) -> Ref<'static, MainThreadGlobals> {
        // We know we're on the main thread because we accept self which is a MainThreadMarker.
        unsafe { GLOBALS.borrow() }
    }

    /// Returns a mutable reference to `MainThreadGlobals`.
    #[inline]
    pub fn globals_mut(self) -> RefMut<'static, MainThreadGlobals> {
        // We know we're on the main thread because we accept self which is a MainThreadMarker.
        unsafe { GLOBALS.borrow_mut() }
    }
}

/// Struct exposing a safe interface to the engine functions.
///
/// There must be only one instance of `Engine` because it also controls access to `CVar`s.
// Not Clone, otherwise the CVar guard can be broken.
pub struct Engine {
    // Mark as !Send and !Sync.
    _marker: PhantomData<*const ()>,
}

/// A guard for statically ensuring that no engine functions are called
/// while the engine `CVar` reference is valid. Holds a mutable reference.
pub struct EngineCVarGuard<'a> {
    engine_cvar: &'a mut cvar_t,
    _borrow_guard: &'a mut Engine,
}

impl Engine {
    /// Creates an `Engine` instance.
    ///
    /// # Safety
    /// Must be called only once from the main game thread to initialize a `static` variable.
    const unsafe fn new() -> Self {
        Self { _marker: PhantomData }
    }

    /// Returns a `MainThreadMarker`.
    ///
    /// Convenience function for not having to pass both the marker and the engine in some cases.
    #[inline]
    pub fn marker(&self) -> MainThreadMarker {
        // We accept the engine by `&self`, which means we're on the main thread.
        unsafe { MainThreadMarker::new() }
    }

    /// Returns an iterator over the console command arguments.
    #[inline]
    pub fn args(&self) -> command::Args<'_> {
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
    // TODO: verify the soundness and interface here. Perhaps accept a cvar_t?
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

    /// Returns the engine `CVar` wrapped by the given `CVar`.
    ///
    /// Takes a mutable reference to `Engine` to statically ensure
    /// that no engine functions are called while the engine `CVar` reference is valid.
    #[inline]
    pub fn get_engine_cvar(&mut self, cvar: &CVar) -> EngineCVarGuard<'_> {
        EngineCVarGuard { engine_cvar: unsafe { cvar.get_engine_cvar() },
                          _borrow_guard: self }
    }
}

impl Deref for EngineCVarGuard<'_> {
    type Target = cvar_t;

    #[inline]
    fn deref(&self) -> &cvar_t {
        self.engine_cvar
    }
}

impl DerefMut for EngineCVarGuard<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut cvar_t {
        self.engine_cvar
    }
}

// fn main_thread_func(_: MainThreadMarker) {}
// fn main_thread_func_with_two_args(_: MainThreadMarker, _a: &mut bool, _b: &mut bool) {}
//
// fn test_partial_borrowing(marker: MainThreadMarker) {
//     // Invoking DerefMut is needed to be able to utilize partial borrows.
//     let globals = &mut *marker.globals();
//
//     let a = &mut globals.capture_sound;
//     let b = &mut globals.inside_gl_setmode;
//     main_thread_func_with_two_args(marker, a, b);
// }
//
// fn test_call_functions(marker: MainThreadMarker) {
//     marker.engine().con_print("Hello");
//     main_thread_func(marker);
//     marker.globals().capture_sound = true;
//
//     let engine = marker.engine();
//     for a in engine.args() {
//         engine.con_print("Arg");
//
//         // If this attempts to get a mutable `Engine` reference, it will panic since we are holding
//         // an immutable reference.
//         main_thread_func(marker);
//         marker.globals().capture_sound = true;
//     }
// }
//
// fn test_cvar_guard(marker: MainThreadMarker) {
//     let engine = marker.engine_mut();
//     let cvar = engine.get_engine_cvar(&crate::capture::cap_fps);
//
//     // Error: can't call engine functions with active cvar references.
//     // engine.con_print("Hi");
//
//     // Can call other main thread funcs.
//     main_thread_func(marker);
//     // Can modify global variables.
//     marker.globals_mut().capture_sound = true;
// }
//
// fn requires_send<T: Send>(_: T) {}
// fn requires_sync<T: Sync>(_: T) {}

// fn check_autotraits(marker: MainThreadMarker) {
//     // Should not compile.
//     requires_send(marker);
//     requires_sync(marker);
//
//     let engine = &mut *marker.engine();
//     requires_send(engine);
//     requires_sync(engine);
// }
