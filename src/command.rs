use crate::engine;

include!(concat!(env!("OUT_DIR"), "/command_array.rs"));

/// An iterator over the console command arguments.
pub struct Args<'a> {
    count: u32,
    index: u32,

    /// Engine functions.
    engine: &'a engine::Engine,
}

impl<'a> Args<'a> {
    #[inline]
    pub fn new(engine: &'a engine::Engine) -> Self {
        Self { count: engine.cmd_argc(),
               index: 0,
               engine: engine }
    }
}

impl<'a> Iterator for Args<'a> {
    type Item = String;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.index == self.count {
            None
        } else {
            let arg = self.engine.cmd_argv(self.index);
            self.index += 1;
            Some(arg)
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        ((self.count - self.index) as usize, Some((self.count - self.index) as usize))
    }
}

impl<'a> ExactSizeIterator for Args<'a> {}

pub trait Command: Send + Sync {
    fn name(&self) -> &'static [u8];
    fn callback(&self) -> unsafe extern "C" fn();
}
