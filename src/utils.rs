use failure::Error;

pub fn format_error(error: &Error) -> String {
    let mut buf = format!("Error: {}\n", error);

    for cause in error.iter_causes() {
        buf += &format!("Caused by: {}\n", cause);
    }

    buf
}
