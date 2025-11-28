use std::fmt;

use serde::{Deserialize, Serialize};

use crate::utils::messages::domain::module as module_messages;

use super::errors::ModuleError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ModuleId(String);

impl ModuleId {
    pub fn new(value: impl Into<String>) -> Result<Self, ModuleError> {
        let trimmed = value.into().trim().to_string();
        if trimmed.is_empty() {
            return Err(ModuleError::Validation(
                module_messages::id::EMPTY.to_string(),
            ));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            return Err(ModuleError::Validation(
                module_messages::id::INVALID_CHARS.to_string(),
            ));
        }
        Ok(Self(trimmed))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
