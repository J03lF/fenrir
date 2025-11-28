use std::fmt;

use crate::utils::messages::domain::module as module_messages;

pub type ModuleResult<T> = Result<T, ModuleServiceError>;

#[derive(Debug)]
pub enum ModuleError {
    Validation(String),
}

#[derive(Debug)]
pub enum ModuleRegistryError {
    Unavailable(String),
    NotFound { module: String },
    Protocol(String),
}

#[derive(Debug)]
pub enum ModuleStorageError {
    Unavailable(String),
    Io(String),
    InvalidState(String),
}

#[derive(Debug)]
pub enum ModuleVerificationError {
    Signature(String),
    Checksum(String),
    Unsupported,
}

#[derive(Debug)]
pub enum ModuleServiceError {
    Registry(ModuleRegistryError),
    Storage(ModuleStorageError),
    Verification(ModuleVerificationError),
}

impl From<ModuleRegistryError> for ModuleServiceError {
    fn from(err: ModuleRegistryError) -> Self {
        Self::Registry(err)
    }
}

impl From<ModuleStorageError> for ModuleServiceError {
    fn from(err: ModuleStorageError) -> Self {
        Self::Storage(err)
    }
}

impl From<ModuleVerificationError> for ModuleServiceError {
    fn from(err: ModuleVerificationError) -> Self {
        Self::Verification(err)
    }
}

impl fmt::Display for ModuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleError::Validation(msg) => f.write_str(&module_messages::validation::error(msg)),
        }
    }
}

impl std::error::Error for ModuleError {}

impl fmt::Display for ModuleRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleRegistryError::Unavailable(msg) => {
                f.write_str(&module_messages::registry_errors::unavailable(msg))
            }
            ModuleRegistryError::NotFound { module } => {
                f.write_str(&module_messages::registry_errors::not_found(module))
            }
            ModuleRegistryError::Protocol(msg) => {
                f.write_str(&module_messages::registry_errors::protocol(msg))
            }
        }
    }
}

impl std::error::Error for ModuleRegistryError {}

impl fmt::Display for ModuleStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleStorageError::Unavailable(msg) => {
                f.write_str(&module_messages::storage_errors::unavailable(msg))
            }
            ModuleStorageError::Io(msg) => f.write_str(&module_messages::storage_errors::io(msg)),
            ModuleStorageError::InvalidState(msg) => {
                f.write_str(&module_messages::storage_errors::invalid_state(msg))
            }
        }
    }
}

impl std::error::Error for ModuleStorageError {}

impl fmt::Display for ModuleVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleVerificationError::Signature(msg) => {
                f.write_str(&module_messages::verification_errors::signature(msg))
            }
            ModuleVerificationError::Checksum(msg) => {
                f.write_str(&module_messages::verification_errors::checksum(msg))
            }
            ModuleVerificationError::Unsupported => {
                f.write_str(module_messages::verification_errors::UNSUPPORTED)
            }
        }
    }
}

impl std::error::Error for ModuleVerificationError {}

impl fmt::Display for ModuleServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModuleServiceError::Registry(err) => {
                f.write_str(&module_messages::service_errors::registry(err))
            }
            ModuleServiceError::Storage(err) => {
                f.write_str(&module_messages::service_errors::storage(err))
            }
            ModuleServiceError::Verification(err) => {
                f.write_str(&module_messages::service_errors::verification(err))
            }
        }
    }
}

impl std::error::Error for ModuleServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ModuleServiceError::Registry(err) => Some(err),
            ModuleServiceError::Storage(err) => Some(err),
            ModuleServiceError::Verification(err) => Some(err),
        }
    }
}
