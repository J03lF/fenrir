use std::{fmt, net::SocketAddr, path::PathBuf, str::FromStr, time::SystemTime};

use thiserror::Error;

use crate::config::ModuleRolloutStrategy;
use crate::domain::module::{
    ModuleId, ModuleInstallResult, ModuleRuntimeError, ModuleRuntimeInfo,
    ModuleRuntimeInstanceInfo, ModuleRuntimeStatus, ModuleVersion,
};
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModuleRuntimeInstanceSnapshot {
    pub instance_id: String,
    pub module_id: ModuleId,
    pub status: ModuleRuntimeStatus,
    pub kind: crate::domain::module::ModuleRuntimeKind,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub endpoint: Option<String>,
    pub started_at: Option<SystemTime>,
    pub stopped_at: Option<SystemTime>,
    pub restart_count: u32,
    pub primary: bool,
}

impl ModuleRuntimeInstanceSnapshot {
    pub fn from_runtime_info(info: ModuleRuntimeInfo) -> Self {
        let instance_id = if let Some(port) = info.port {
            format!("{}:port:{port}", info.module_id)
        } else if let Some(pid) = info.pid {
            format!("{}:pid:{pid}", info.module_id)
        } else {
            format!("{}:primary", info.module_id)
        };
        let endpoint = info.port.map(|port| format!("http://127.0.0.1:{port}"));
        Self {
            instance_id,
            module_id: info.module_id,
            status: info.status,
            kind: info.kind,
            pid: info.pid,
            port: info.port,
            endpoint,
            started_at: info.started_at,
            stopped_at: info.stopped_at,
            restart_count: info.restart_count,
            primary: true,
        }
    }

    pub fn from_instance_info(instance: ModuleRuntimeInstanceInfo) -> Self {
        let endpoint = instance
            .runtime
            .port
            .map(|port| format!("http://127.0.0.1:{port}"));
        Self {
            instance_id: instance.instance_id,
            module_id: instance.runtime.module_id,
            status: instance.runtime.status,
            kind: instance.runtime.kind,
            pid: instance.runtime.pid,
            port: instance.runtime.port,
            endpoint,
            started_at: instance.runtime.started_at,
            stopped_at: instance.runtime.stopped_at,
            restart_count: instance.runtime.restart_count,
            primary: instance.primary,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModuleRollingRestartReport {
    pub module_id: ModuleId,
    pub restarted_instances: Vec<ModuleRuntimeInstanceSnapshot>,
    pub health_verified: bool,
    pub note: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModuleCanaryRoutingStatus {
    pub module_id: ModuleId,
    pub strategy: Option<ModuleRolloutStrategy>,
    pub traffic_percent: u8,
    pub configured_instances: Vec<String>,
    pub active_canary_instances: Vec<ModuleRuntimeInstanceSnapshot>,
    pub active_stable_instances: Vec<ModuleRuntimeInstanceSnapshot>,
}

#[derive(Debug, Clone)]
pub enum ModuleIngressTarget {
    RuntimePort {
        module_id: ModuleId,
        instance_id: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModuleScaffoldRuntime {
    #[default]
    Rust,
    Node,
    Angular,
}

impl ModuleScaffoldRuntime {
    pub const fn variants() -> &'static [&'static str] {
        &["rust", "node", "angular"]
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            ModuleScaffoldRuntime::Rust => "rust",
            ModuleScaffoldRuntime::Node => "node",
            ModuleScaffoldRuntime::Angular => "angular",
        }
    }
}

impl fmt::Display for ModuleScaffoldRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ModuleScaffoldRuntime {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rust" => Ok(Self::Rust),
            "node" | "ts" | "typescript" => Ok(Self::Node),
            "angular" | "ng" => Ok(Self::Angular),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModuleScaffoldOptions {
    pub runtime: ModuleScaffoldRuntime,
}

impl ModuleScaffoldOptions {
    pub const fn new(runtime: ModuleScaffoldRuntime) -> Self {
        Self { runtime }
    }
}

impl Default for ModuleScaffoldOptions {
    fn default() -> Self {
        Self {
            runtime: ModuleScaffoldRuntime::Rust,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModuleScaffoldSummary {
    pub module_id: ModuleId,
    pub runtime: ModuleScaffoldRuntime,
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleStartupTrigger {
    Autostart,
    EnsureRunning,
    ManualStart,
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleStartupStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleStartupPhaseStatus {
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModuleStartupPhaseReport {
    pub phase: String,
    pub status: ModuleStartupPhaseStatus,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModuleStartupReport {
    pub module_id: ModuleId,
    pub trigger: ModuleStartupTrigger,
    pub status: ModuleStartupStatus,
    pub started_at: String,
    pub completed_at: String,
    pub total_duration_ms: u64,
    pub phases: Vec<ModuleStartupPhaseReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleOverrideReloadStatus {
    Applied,
    RolledBack,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleOverrideReloadAction {
    DescriptorRefreshed,
    Reconciled,
    Restarted,
    Skipped,
    Unchanged,
}

#[derive(Debug, Clone)]
pub struct ModuleOverrideReloadModuleReport {
    pub module_id: ModuleId,
    pub action: ModuleOverrideReloadAction,
    pub env_changed: bool,
    pub health_checked: bool,
    pub healthy: bool,
    pub rolled_back: bool,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct ModuleOverrideReloadReport {
    pub status: ModuleOverrideReloadStatus,
    pub restart_running: bool,
    pub restarted_modules: Vec<ModuleId>,
    pub rollback_restarted_modules: Vec<ModuleId>,
    pub modules: Vec<ModuleOverrideReloadModuleReport>,
}

// ============================================================================
// Services JSON Support
// ============================================================================

/// JSON manifest for module services (`.fenrir/services.json`).
/// This format is compatible with `fenrir-module-kit` and allows modules
/// to declare their services statically.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModuleServicesJson {
    /// Schema version (e.g., "1.0")
    pub schema_version: String,
    /// List of services provided by this module
    #[serde(default)]
    pub services: Vec<ModuleServiceJsonEntry>,
}

/// A single service entry in the services JSON manifest.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModuleServiceJsonEntry {
    /// Service identifier (e.g., "api-gateway", "core", "auth")
    pub service_id: String,
    /// Human-readable service name
    #[serde(default)]
    pub name: Option<String>,
    /// Service description
    #[serde(default)]
    pub description: Option<String>,
    /// Service kind (e.g., "http", "grpc", "transport")
    #[serde(default)]
    pub kind: Option<String>,
    /// Route prefix for gateway routing (e.g., "/api/v1")
    #[serde(default)]
    pub route_prefix: Option<String>,
    /// Health check endpoint path (e.g., "/health")
    #[serde(default)]
    pub health_path: Option<String>,
    /// Whether service is internal-only (not exposed via gateway)
    #[serde(default)]
    pub internal_only: Option<bool>,
    /// Ingress access level ("internal" or "public")
    #[serde(default)]
    pub ingress_access: Option<String>,
    /// Supported protocols (e.g., ["http", "grpc"])
    #[serde(default)]
    pub protocols: Vec<String>,
    /// Required scopes for access (e.g., ["athene:read", "athene:write"])
    #[serde(default)]
    pub required_scopes: Vec<String>,
    /// Allowed roles for access (e.g., ["admin", "operator"])
    #[serde(default)]
    pub allowed_roles: Vec<String>,
    /// Rate limit per second (if specified)
    #[serde(default)]
    pub rate_limit_per_second: Option<u32>,
    /// Whether to disable rate limiting entirely
    #[serde(default)]
    pub disable_rate_limit: Option<bool>,
}
