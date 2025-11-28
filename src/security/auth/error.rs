use std::error::Error as StdError;
use std::fmt;

use crate::utils::messages::security::auth as auth_messages;

#[derive(Debug)]
pub enum AuthError {
    Unauthorized,
    Forbidden,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthError::Unauthorized => f.write_str(auth_messages::unauthorized()),
            AuthError::Forbidden => f.write_str(auth_messages::forbidden()),
        }
    }
}

impl StdError for AuthError {}
