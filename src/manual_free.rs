use std::ops::{Deref, DerefMut};

pub struct ManualFree<T> {
    ptr: *mut T,
    freed: bool,
}

impl<T> ManualFree<T> {
    #[inline]
    pub fn new(x: T) -> Self {
        Self { ptr: Box::into_raw(Box::new(x)),
               freed: false, }
    }

    #[inline]
    pub fn free(&mut self) {
        if !self.freed {
            drop(unsafe { Box::from_raw(self.ptr) });
            self.freed = true;
        }
    }
}

impl<T> Deref for ManualFree<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        if self.freed {
            panic!("use after free");
        }

        unsafe { &*self.ptr }
    }
}

impl<T> DerefMut for ManualFree<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        if self.freed {
            panic!("use after free");
        }

        unsafe { &mut *self.ptr }
    }
}
