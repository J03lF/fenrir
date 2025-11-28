use std::fmt;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::utils::messages::domain::module as module_messages;

use super::errors::ModuleError;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModuleVersion(pub Version);

impl ModuleVersion {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ModuleError> {
        let version = Version::parse(value.as_ref()).map_err(|err| {
            ModuleError::Validation(module_messages::version::invalid(value.as_ref(), err))
        })?;
        Ok(Self(version))
    }

    pub fn as_semver(&self) -> &Version {
        &self.0
    }
}

impl fmt::Display for ModuleVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Serialize for ModuleVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ModuleVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Version::parse(&raw)
            .map(ModuleVersion)
            .map_err(serde::de::Error::custom)
    }
}
