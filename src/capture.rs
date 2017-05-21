use std::sync::{RwLock, mpsc};

lazy_static! {
    static ref CAPTURING: RwLock<bool> = RwLock::new(false);
}

pub fn is_capturing() -> bool {
    *CAPTURING.read().unwrap()
}

command!(cap_start, |_engine| {
    *CAPTURING.write().unwrap() = true;
});

command!(cap_stop, |_engine| {
    *CAPTURING.write().unwrap() = false;
});
