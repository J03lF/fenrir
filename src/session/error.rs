use std::error::Error as StdError;
use std::fmt;

use crate::utils::messages::session::errors as session_errors;

#[derive(Debug, Clone)]
pub enum SessionError {
    Invalid(String),
    Storage(String),
}

impl SessionError {
    #[allow(dead_code)]
    pub fn storage<E: fmt::Display>(err: E) -> Self {
        SessionError::Storage(err.to_string())
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Invalid(reason) => write!(f, "{}", session_errors::invalid(reason)),
            SessionError::Storage(reason) => write!(f, "{}", session_errors::storage(reason)),
        }
    }
}

impl StdError for SessionError {}
