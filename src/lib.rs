// For error_chain.
#![recursion_limit = "1024"]

#[macro_use]
extern crate error_chain;
extern crate ffmpeg;
extern crate fine_grained;
#[macro_use]
extern crate lazy_static;
extern crate libc;
extern crate gl;
extern crate glx;
extern crate ocl;
extern crate sdl2_sys;

mod errors {
    // Create the Error, ErrorKind, ResultExt, and Result types.
    error_chain!{}
}
#[macro_use]
mod macros;
mod capture;
mod command;
mod cvar;
mod dl;
mod encode;
mod engine;
mod hooks {
    pub mod hw;
}
// mod profiler;
mod sdl;

pub use self::hooks::hw::RunListenServer;
pub use self::hooks::hw::CL_Disconnect;
pub use self::hooks::hw::Con_ToggleConsole_f;
pub use self::hooks::hw::GL_EndRendering;
pub use self::hooks::hw::Host_FilterTime;
pub use self::hooks::hw::Key_Event;
pub use self::hooks::hw::Memory_Init;
pub use self::hooks::hw::S_PaintChannels;
pub use self::hooks::hw::S_TransferStereo16;
pub use self::hooks::hw::Sys_VID_FlipScreen;
