macro_rules! cstr {
    ($s:expr) => ($s as *const _ as *const libc::c_char)
}
