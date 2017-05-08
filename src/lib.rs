// For error_chain.
#![recursion_limit = "1024"]

#[macro_use]
extern crate error_chain;
#[macro_use]
extern crate lazy_static;
extern crate libc;

mod errors {
    // Create the Error, ErrorKind, ResultExt, and Result types
    error_chain!{}
}
#[macro_use]
mod macros;
mod command;
mod dl;
mod function;
mod hooks {
    pub mod hw;
}

pub use self::hooks::hw::RunListenServer;
pub use self::hooks::hw::Memory_Init;
