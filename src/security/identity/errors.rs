use std::error::Error as StdError;
use std::fmt;

use crate::security::auth::AuthError;
use crate::utils::messages::security::identity as identity_messages;
#[derive(Debug)]
pub enum IdentityError {
    Io(std::io::Error),
    Serde(serde_json::Error),
    StatePoisoned,
    Invalid(String),
    Unauthorized(String),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityError::Io(err) => f.write_str(&identity_messages::store_io_error(err)),
            IdentityError::Serde(err) => {
                f.write_str(&identity_messages::store_serialization_error(err))
            }
            IdentityError::StatePoisoned => f.write_str(identity_messages::state_poisoned()),
            IdentityError::Invalid(reason) => f.write_str(&identity_messages::data_invalid(reason)),
            IdentityError::Unauthorized(reason) => {
                f.write_str(&identity_messages::authorization_failed(reason))
            }
        }
    }
}

impl StdError for IdentityError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            IdentityError::Io(err) => Some(err),
            IdentityError::Serde(err) => Some(err),
            _ => None,
        }
    }
}

impl From<IdentityError> for AuthError {
    fn from(err: IdentityError) -> Self {
        match err {
            IdentityError::Unauthorized(_) => AuthError::Unauthorized,
            _ => AuthError::Forbidden,
        }
    }
}
