use failure::Fail;

pub mod racy_ref_cell;
pub use self::racy_ref_cell::RacyRefCell;

pub mod maybe_unavaliable;
pub use self::maybe_unavaliable::MaybeUnavailable;

/// Returns a string describing the error and the full chain.
pub fn format_error(fail: &dyn Fail) -> String {
    let mut buf = format!("Error: {}\n", fail);

    for cause in fail.iter_causes() {
        buf += &format!("Caused by: {}\n", cause);
    }

    buf
}
