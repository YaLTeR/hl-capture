use libc::*;
use std::ffi::CStr;
use std::sync::RwLock;

lazy_static! {
    pub static ref COMMANDS: RwLock<Vec<Box<Command>>> = RwLock::new(Vec::new());
}

pub struct Args {
    count: usize,
    index: usize,
}

impl Args {
    fn new() -> Self {
        Self {
            count: real!(Cmd_Argc)() as usize,
            index: 0,
        }
    }
}

impl Iterator for Args {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index == self.count {
            None
        } else {
            let arg = real!(Cmd_Argv)(self.index as c_int);
            self.index += 1;

            let string = unsafe { CStr::from_ptr(arg).to_string_lossy().into_owned() };
            Some(string)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count - self.index, Some(self.count - self.index))
    }
}

impl ExactSizeIterator for Args {}

pub type ArgsMaker = Fn() -> Args;
pub const MAKE_ARGS: &ArgsMaker = &|| Args::new();

pub trait Command: Send + Sync {
    fn name(&self) -> &'static [u8];
    fn callback(&self) -> extern "C" fn();
}
