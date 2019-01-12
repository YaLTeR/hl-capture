use failure::Error;

pub mod maybe_unavaliable;
pub use self::maybe_unavaliable::MaybeUnavailable;

/// Returns a string describing the error and the full chain.
pub fn format_error(error: &Error) -> String {
    let mut buf = format!("Error: {}\n", error);

    for cause in error.iter_causes() {
        buf += &format!("Caused by: {}\n", cause);
    }

    buf
}
