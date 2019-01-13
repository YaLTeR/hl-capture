use crate::engine::Engine;

include!(concat!(env!("OUT_DIR"), "/command_array.rs"));

/// An iterator over the console command arguments.
pub struct Args<'a> {
    count: u32,
    index: u32,

    /// Engine functions.
    engine: &'a Engine,
}

impl<'a> Args<'a> {
    #[inline]
    pub fn new(engine: &'a Engine) -> Self {
        Self { count: engine.cmd_argc(),
               index: 0,
               engine: engine }
    }
}

impl Iterator for Args<'_> {
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

impl ExactSizeIterator for Args<'_> {}

pub trait Command: Send + Sync {
    fn name(&self) -> &'static [u8];
    fn callback(&self) -> unsafe extern "C" fn();
}
