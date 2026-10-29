//! Building blocks for installs whose content was verified elsewhere (the
//! Fenrir Depot pipeline in `services::depot`): activate a bundle as is,
//! snapshot an installed module, and wait for a module to be healthy.
//!
//! They reuse the storage, runtime and health mechanics of the regular
//! install path; what differs is only where the trust comes from (a signed
//! distribution manifest instead of a per-module signature).

use std::path::PathBuf;

use crate::domain::module::{
    ModuleBundle, ModuleId, ModuleInstallResult, ModuleInstallSource, ModuleManifest, ModuleResult,
    ModuleRuntimeStatus, ModuleServiceError, ModuleStorageError,
};
use crate::services::ServiceStatus;
use crate::utils::messages::services::module::service::notes as module_service_notes;

use super::dev::package_module_directory;
use super::service::ModuleService;

impl ModuleService {
    /// Replaces the files of a module with `bundle`, whose archive the
    /// caller has verified. A lower version than the installed one is
    /// allowed (rollback): the old files are removed first, which is what
    /// staging does anyway. The module is not started; the caller stops it
    /// before (or restarts it after, rolling).
    pub async fn activate_verified_bundle(
        &self,
        bundle: ModuleBundle,
    ) -> ModuleResult<ModuleInstallResult> {
        let module_id = bundle.manifest.module_id().map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
        })?;
        if let Some(installed) = self.storage.load(&module_id).await? {
            if installed.manifest.module_version() > bundle.manifest.module_version() {
                self.storage.remove(&module_id).await?;
            }
        }
        let result = self
            .storage
            .stage_and_activate(bundle, ModuleInstallSource::Distribution)
            .await?;
        self.update_module_service_status(
            &module_id,
            &result.manifest,
            ServiceStatus::Standby,
            Some(module_service_notes::INSTALLED.to_string()),
        );
        Ok(result)
    }

    /// The installed manifest and a gzip tar of the module directory (the
    /// same packaging as the distribution backup of `synchronize`), or
    /// `None` when the module is not installed.
    pub async fn package_installed_module(
        &self,
        module_id: &ModuleId,
    ) -> ModuleResult<Option<(ModuleManifest, Vec<u8>)>> {
        let Some(installed) = self.storage.load(module_id).await? else {
            return Ok(None);
        };
        let archive = package_module_directory(PathBuf::from(&installed.path)).await?;
        Ok(Some((installed.manifest, archive)))
    }

    /// Stops the module's processes (if any) the way an update does.
    pub async fn stop_for_replacement(&self, module_id: &ModuleId) {
        self.stop_module_process(module_id).await;
    }

    /// Waits until the module runs and its health endpoint answers, within
    /// `modules.runtime.rollout.health_check_timeout_ms`. A module without a
    /// routable port (static or worker) counts as healthy once it runs.
    pub async fn wait_until_healthy(&self, module_id: &ModuleId) -> Result<String, String> {
        let info = self
            .runtime_status(module_id)
            .await
            .map_err(|err| err.to_string())?;
        if !matches!(info.status, ModuleRuntimeStatus::Running) {
            return Err(format!("{module_id} is not running ({:?})", info.status));
        }
        self.verify_module_rollout_health(module_id, info.port)
            .await
    }
}
