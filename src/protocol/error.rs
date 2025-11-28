use std::error::Error as StdError;
use std::fmt;

use crate::utils::messages::protocol as protocol_messages;

#[derive(Debug)]
pub enum ProtocolError {
    Serialization(serde_json::Error),
    VersionMismatch(u16),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::Serialization(err) => {
                f.write_str(&protocol_messages::serialization_failed(err))
            }
            ProtocolError::VersionMismatch(version) => {
                f.write_str(&protocol_messages::version_mismatch(*version))
            }
        }
    }
}

impl StdError for ProtocolError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            ProtocolError::Serialization(err) => Some(err),
            ProtocolError::VersionMismatch(_) => None,
        }
    }
}
