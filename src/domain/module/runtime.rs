use std::time::SystemTime;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::utils::messages::domain::module as module_messages;

use super::errors::{ModuleServiceError, ModuleStorageError};
use super::id::ModuleId;
use super::version::ModuleVersion;

/// Status of a running module instance
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleRuntimeStatus {
    /// Module is currently running
    Running,
    /// Module is stopped
    Stopped,
    /// Module failed to start or crashed
    Failed,
    /// Module is starting up
    Starting,
    /// Module is stopping
    Stopping,
}

/// Information about a running module instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleRuntimeInfo {
    pub module_id: ModuleId,
    pub version: ModuleVersion,
    pub status: ModuleRuntimeStatus,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub started_at: Option<SystemTime>,
    pub stopped_at: Option<SystemTime>,
    pub restart_count: u32,
}

/// Configuration for starting a module
#[derive(Debug, Clone)]
pub struct ModuleStartConfig {
    pub module_id: ModuleId,
    pub port: Option<u16>,
    pub env_vars: Vec<(String, String)>,
    pub auto_restart: bool,
}

/// Port trait for module runtime management
#[async_trait]
pub trait ModuleRuntimePort: Send + Sync {
    /// Start a module instance
    async fn start(
        &self,
        config: ModuleStartConfig,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError>;

    /// Stop a running module instance
    async fn stop(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError>;

    /// Get status of a module instance
    async fn status(&self, module_id: &ModuleId) -> Result<ModuleRuntimeInfo, ModuleRuntimeError>;

    /// List all running modules
    async fn list_running(&self) -> Result<Vec<ModuleRuntimeInfo>, ModuleRuntimeError>;

    /// Restart a module instance
    async fn restart(&self, module_id: &ModuleId) -> Result<ModuleRuntimeInfo, ModuleRuntimeError>;

    /// Get logs from a module instance
    async fn logs(
        &self,
        module_id: &ModuleId,
        tail: Option<usize>,
    ) -> Result<Vec<String>, ModuleRuntimeError>;
}

/// Errors that can occur during module runtime operations
#[derive(Debug)]
pub enum ModuleRuntimeError {
    NotInstalled { module_id: String },

    AlreadyRunning { module_id: String },

    NotRunning { module_id: String },

    StartFailed { module_id: String, reason: String },

    StopFailed { module_id: String, reason: String },

    PortInUse { port: u16 },

    Io(String),

    InvalidState(String),
}

impl From<ModuleRuntimeError> for ModuleServiceError {
    fn from(err: ModuleRuntimeError) -> Self {
        ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
    }
}

impl std::fmt::Display for ModuleRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleRuntimeError::NotInstalled { module_id } => {
                f.write_str(&module_messages::runtime_errors::not_installed(module_id))
            }
            ModuleRuntimeError::AlreadyRunning { module_id } => {
                f.write_str(&module_messages::runtime_errors::already_running(module_id))
            }
            ModuleRuntimeError::NotRunning { module_id } => {
                f.write_str(&module_messages::runtime_errors::not_running(module_id))
            }
            ModuleRuntimeError::StartFailed { module_id, reason } => f.write_str(
                &module_messages::runtime_errors::start_failed(module_id, reason),
            ),
            ModuleRuntimeError::StopFailed { module_id, reason } => f.write_str(
                &module_messages::runtime_errors::stop_failed(module_id, reason),
            ),
            ModuleRuntimeError::PortInUse { port } => {
                f.write_str(&module_messages::runtime_errors::port_in_use(*port))
            }
            ModuleRuntimeError::Io(message) => {
                f.write_str(&module_messages::runtime_errors::io_error(message))
            }
            ModuleRuntimeError::InvalidState(message) => {
                f.write_str(&module_messages::runtime_errors::invalid_state(message))
            }
        }
    }
}

impl std::error::Error for ModuleRuntimeError {}
