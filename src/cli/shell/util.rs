use rustyline::error::ReadlineError;
use std::io;

pub(crate) fn map_readline_error(err: ReadlineError) -> io::Error {
    match err {
        ReadlineError::Io(err) => err,
        other => io::Error::other(other.to_string()),
    }
}
