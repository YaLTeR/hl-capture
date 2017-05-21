// For error_chain.
#![recursion_limit = "1024"]

#[macro_use]
extern crate error_chain;
#[macro_use]
extern crate ffmpeg;
#[macro_use]
extern crate lazy_static;
extern crate libc;
extern crate gl;
extern crate sdl2_sys;

mod errors {
    // Create the Error, ErrorKind, ResultExt, and Result types.
    error_chain!{}
}
#[macro_use]
mod macros;
mod capture;
mod command;
mod dl;
mod encode;
mod engine;
mod function;
mod hooks {
    pub mod hw;
}
mod sdl;

type Frame = ffmpeg::frame::Video;

pub use self::hooks::hw::RunListenServer;
pub use self::hooks::hw::Memory_Init;
pub use self::hooks::hw::Sys_VID_FlipScreen;
