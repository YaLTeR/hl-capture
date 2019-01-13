use std::mem;

/// Represents a value that is not checked at first, and upon being checked it might be available
/// or unavailable.
pub enum MaybeUnavailable<T> {
    // I'm not that great at naming.
    NotChecked,
    Unavailable,
    Available(T),
}

impl<T> MaybeUnavailable<T> {
    /// Resets this value to the not checked state.
    #[inline]
    pub fn reset(&mut self) {
        *self = MaybeUnavailable::NotChecked;
    }

    /// If the value is available, returns it, otherwise returns `None`.
    #[inline]
    pub fn available(self) -> Option<T> {
        match self {
            MaybeUnavailable::Available(x) => Some(x),
            _ => None,
        }
    }

    /// Takes a value out of the `MaybeUnavailable`, leaving `NotChecked` in its place if it was
    /// `Available` and retains the value otherwise.
    #[inline]
    pub fn take(&mut self) -> Option<T> {
        let new_value = match self {
            MaybeUnavailable::NotChecked => MaybeUnavailable::NotChecked,
            MaybeUnavailable::Unavailable => MaybeUnavailable::Unavailable,
            MaybeUnavailable::Available(_) => MaybeUnavailable::NotChecked,
        };

        mem::replace(self, new_value).available()
    }

    /// Moves the value `v` out of the `MaybeUnavailable<T>` if it is `Available(v)`.
    ///
    /// # Panics
    ///
    /// Panics if the self value does not equal `Available(v)`.
    #[inline]
    pub fn unwrap(self) -> T {
        match self {
            MaybeUnavailable::Available(x) => x,
            MaybeUnavailable::NotChecked => {
                panic!("called `MaybeUnavailable::unwrap()` on a `NotChecked` value")
            }
            MaybeUnavailable::Unavailable => {
                panic!("called `MaybeUnavailable::unwrap()` on a `Unavailable` value")
            }
        }
    }

    /// Returns `true` if the value is `NotChecked`.
    #[inline]
    pub fn is_not_checked(&self) -> bool {
        match self {
            MaybeUnavailable::NotChecked => true,
            _ => false,
        }
    }

    /// Returns `true` if the value is `Available`.
    #[inline]
    pub fn is_available(&self) -> bool {
        match self {
            MaybeUnavailable::Available(_) => true,
            _ => false,
        }
    }

    // /// Converts from `MaybeUnavailable<T>` to `MaybeUnavailable<&T>`.
    // #[inline]
    // pub fn as_ref(&self) -> MaybeUnavailable<&T> {
    //     match *self {
    //         MaybeUnavailable::NotChecked => MaybeUnavailable::NotChecked,
    //         MaybeUnavailable::Unavailable => MaybeUnavailable::Unavailable,
    //         MaybeUnavailable::Available(ref x) => MaybeUnavailable::Available(x),
    //     }
    // }

    /// Converts from `MaybeUnavailable<T>` to `MaybeUnavailable<&mut T>`.
    #[inline]
    pub fn as_mut(&mut self) -> MaybeUnavailable<&mut T> {
        match *self {
            MaybeUnavailable::NotChecked => MaybeUnavailable::NotChecked,
            MaybeUnavailable::Unavailable => MaybeUnavailable::Unavailable,
            MaybeUnavailable::Available(ref mut x) => MaybeUnavailable::Available(x),
        }
    }

    /// Returns `Available(x)` if passed `Some(x)` and `Unavailable` otherwise.
    #[inline]
    pub fn from_check_result(x: Option<T>) -> Self {
        match x {
            Some(x) => MaybeUnavailable::Available(x),
            None => MaybeUnavailable::Unavailable,
        }
    }
}
