#[macro_use]
extern crate failure;
extern crate ffmpeg;
extern crate fine_grained;
extern crate gl;
extern crate glx;
#[macro_use]
extern crate lazy_static;
extern crate libc;
extern crate ocl;
extern crate sdl2_sys;

#[macro_use]
mod macros;
mod capture;
mod command;
mod cvar;
mod dl;
mod encode;
mod engine;
mod fps_converter;
mod hooks {
    pub mod hw;
}
// mod profiler;
mod sdl;
mod utils;

#[link(name = "GL", kind = "dylib")]
extern "C" {}

pub use self::hooks::hw::CL_Disconnect;
pub use self::hooks::hw::Con_ToggleConsole_f;
pub use self::hooks::hw::GL_SetMode;
pub use self::hooks::hw::Host_FilterTime;
pub use self::hooks::hw::Key_Event;
pub use self::hooks::hw::Memory_Init;
pub use self::hooks::hw::RunListenServer;
pub use self::hooks::hw::S_PaintChannels;
pub use self::hooks::hw::S_TransferStereo16;
pub use self::hooks::hw::Sys_VID_FlipScreen;
pub use self::hooks::hw::VideoMode_IsWindowed;
