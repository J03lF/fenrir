use std::net::SocketAddr;
use std::path::PathBuf;

use thiserror::Error;

use crate::domain::module::{ModuleId, ModuleInstallResult, ModuleRuntimeError, ModuleVersion};
use crate::services::{ServiceIngressMetadata, ServiceKind, ServiceSecurityMetadata};
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
pub struct ModuleReleaseOutcome {
    pub install_result: ModuleInstallResult,
    pub dev_override_cleared: bool,
}

#[derive(Debug, Clone)]
pub struct ModuleDevServices {
    pub module_id: ModuleId,
    pub version: ModuleVersion,
    pub services: Vec<RegisteredDevService>,
    pub run: Option<ModuleDevRunState>,
}

#[derive(Debug, Clone)]
pub struct ModuleDevRunState {
    pub command: String,
    pub workdir: PathBuf,
    pub auto_restart: bool,
    pub auto_start: bool,
    pub log_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RegisteredDevService {
    pub service_id: String,
    pub endpoint: SocketAddr,
    pub name: String,
    pub description: Option<String>,
    pub kind: ServiceKind,
    pub security: Option<ServiceSecurityMetadata>,
    pub ingress: Option<ServiceIngressMetadata>,
}

#[derive(Debug, Clone)]
pub enum ModuleIngressTarget {
    RuntimePort {
        module_id: ModuleId,
        port: u16,
    },
    DevService {
        module_id: ModuleId,
        service_id: String,
        endpoint: SocketAddr,
    },
    DeclaredService {
        module_id: ModuleId,
        service_id: String,
        endpoint: SocketAddr,
    },
}

impl ModuleIngressTarget {
    pub fn module_id(&self) -> &ModuleId {
        match self {
            ModuleIngressTarget::RuntimePort { module_id, .. }
            | ModuleIngressTarget::DevService { module_id, .. }
            | ModuleIngressTarget::DeclaredService { module_id, .. } => module_id,
        }
    }
}

#[derive(Debug, Error)]
pub enum ModuleIngressError {
    #[error("service '{0}' is not managed by the module runtime")]
    UnsupportedService(String),
    #[error("module '{0}' is not running")]
    ModuleNotRunning(String),
    #[error("module '{0}' has no runtime port assigned")]
    ModulePortUnknown(String),
    #[error("module '{module_id}' has no active override service '{service_id}'")]
    DevServiceInactive {
        module_id: String,
        service_id: String,
    },
    #[error("module '{module_id}' has no declared service '{service_id}'")]
    DeclaredServiceMissing {
        module_id: String,
        service_id: String,
    },
    #[error("invalid module id '{0}'")]
    InvalidModuleId(String),
    #[error("module runtime error: {0}")]
    Runtime(#[from] ModuleRuntimeError),
}
