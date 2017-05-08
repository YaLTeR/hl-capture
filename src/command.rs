use std::sync::RwLock;

lazy_static! {
    pub static ref COMMANDS: RwLock<Vec<Box<Command>>> = RwLock::new(Vec::new());
}

pub struct Args {
    index: usize,
}

impl Args {
    fn new() -> Self {
        Self {
            index: 0,
        }
    }
}

impl Iterator for Args {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}

impl ExactSizeIterator for Args {}

pub type ArgsMaker = Fn() -> Args;
pub const MAKE_ARGS: &ArgsMaker = &|| Args::new();

pub trait Command: Send + Sync {
    fn name(&self) -> &'static [u8];
    fn callback(&self) -> extern "C" fn();
}
