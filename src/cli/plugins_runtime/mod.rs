use std::sync::Arc;

use tokio::runtime::Handle;
use tracing::debug;

use crate::domain::module::{InstalledModule, ModuleServiceError};
use crate::services::ModuleService;

#[derive(Clone)]
pub struct PluginRuntime {
    module_service: Arc<ModuleService>,
}

impl PluginRuntime {
    pub fn new(module_service: Arc<ModuleService>) -> Self {
        Self { module_service }
    }

    pub async fn sync(&self) -> Result<Vec<InstalledModule>, ModuleServiceError> {
        let modules = self.module_service.list_installed().await?;
        debug!(count = modules.len(), "plugin runtime synced installed modules");
        Ok(modules)
    }

    pub fn sync_blocking(&self) -> Result<Vec<InstalledModule>, PluginRuntimeError> {
        let handle = Handle::try_current().map_err(|_| PluginRuntimeError::RuntimeUnavailable)?;
        let service = Arc::clone(&self.module_service);
        handle
            .block_on(async move { service.list_installed().await })
            .map_err(PluginRuntimeError::from)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum PluginRuntimeError {
    #[error("tokio runtime handle not available")]
    RuntimeUnavailable,
    #[error(transparent)]
    Module(#[from] ModuleServiceError),
}
