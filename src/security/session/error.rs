use std::error::Error as StdError;
use std::fmt;

use crate::utils::messages::security::session as session_messages;

#[derive(Debug)]
pub enum SessionError {
    NotFound,
    Expired,
    IdleTimeout,
    Store,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::NotFound => f.write_str(session_messages::not_found()),
            SessionError::Expired => f.write_str(session_messages::expired()),
            SessionError::IdleTimeout => f.write_str(session_messages::idle_timeout()),
            SessionError::Store => f.write_str(session_messages::store_unavailable()),
        }
    }
}

impl StdError for SessionError {}

impl<T> From<std::sync::PoisonError<T>> for SessionError {
    fn from(_: std::sync::PoisonError<T>) -> Self {
        SessionError::Store
    }
}
