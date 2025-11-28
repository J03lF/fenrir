use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::utils::messages::domain::module as module_messages;

use super::id::ModuleId;
use super::manifest::ModuleManifest;
use super::version::ModuleVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleInstallSource {
    Distribution,
    LocalOverride,
}

impl ModuleInstallSource {
    pub fn is_synchronized(&self) -> bool {
        matches!(self, Self::LocalOverride)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Distribution => module_messages::install::DISTRIBUTION_LABEL,
            Self::LocalOverride => module_messages::install::LOCAL_OVERRIDE_LABEL,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstalledModule {
    pub manifest: ModuleManifest,
    pub installed_at: SystemTime,
    pub path: String,
    pub source: ModuleInstallSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleInstallStatus {
    Installed,
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone)]
pub struct ModuleInstallResult {
    pub status: ModuleInstallStatus,
    pub manifest: ModuleManifest,
    pub path: String,
    pub source: ModuleInstallSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistributionTarget {
    pub module_id: ModuleId,
    pub version: ModuleVersion,
}
