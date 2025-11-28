use std::net::SocketAddr;

use crate::domain::module::{ModuleId, ModuleInstallResult, ModuleVersion};
use crate::services::ServiceKind;
use crate::utils::messages::services::module::types::distribution_action;

#[derive(Debug, Clone)]
pub struct ModuleUpdateInfo {
    pub module_id: ModuleId,
    pub current_version: ModuleVersion,
    pub latest_version: ModuleVersion,
    pub has_update: bool,
    pub compatible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DistributionAction {
    Install,
    Update,
    AlreadyCurrent,
}

impl DistributionAction {
    pub fn label(&self) -> &'static str {
        match self {
            DistributionAction::Install => distribution_action::INSTALL,
            DistributionAction::Update => distribution_action::UPDATE,
            DistributionAction::AlreadyCurrent => distribution_action::ALREADY_CURRENT,
        }
    }

    pub fn requires_execution(&self) -> bool {
        matches!(
            self,
            DistributionAction::Install | DistributionAction::Update
        )
    }
}

#[derive(Debug, Clone)]
pub struct DistributionPlanEntry {
    pub module_id: ModuleId,
    pub target_version: ModuleVersion,
    pub current_version: Option<ModuleVersion>,
    pub action: DistributionAction,
}

#[derive(Debug, Clone)]
pub enum ModuleSyncOutcome {
    Packaged(Box<ModuleSyncPackage>),
    ExternalServices(ModuleDevServices),
}

#[derive(Debug, Clone)]
pub struct ModuleSyncPackage {
    pub install_result: ModuleInstallResult,
    pub packaged_from: std::path::PathBuf,
}

#[derive(Debug, Clone)]
pub struct ModuleDevServices {
    pub module_id: ModuleId,
    pub version: ModuleVersion,
    pub services: Vec<RegisteredDevService>,
}

#[derive(Debug, Clone)]
pub struct RegisteredDevService {
    pub service_id: String,
    pub endpoint: SocketAddr,
    pub name: String,
    pub description: Option<String>,
    pub kind: ServiceKind,
}
