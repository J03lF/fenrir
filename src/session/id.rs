use std::fmt;
use std::str::FromStr;

use uuid::Uuid;

use crate::utils::messages::session::errors as session_errors;

use super::error::SessionError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(Uuid);

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SessionId {
    type Err = SessionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s)
            .map(SessionId)
            .map_err(|_| SessionError::Invalid(session_errors::INVALID_ID.to_string()))
    }
}
