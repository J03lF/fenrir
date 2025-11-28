use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::domain::module::{
    DistributionTarget, InstalledModule, ModuleId, ModuleInstallResult, ModuleInstallSource,
    ModuleInstallStatus, ModuleManifest, ModuleRegistryPort, ModuleResult, ModuleRuntimePort,
    ModuleSearchQuery, ModuleServiceError, ModuleStorageError, ModuleStoragePort, ModuleSummary,
    ModuleVerifierPort, ModuleVersion, ProgressCallback,
};
use crate::services::{ServiceDescriptorOwned, ServiceKind, ServiceRegistry, ServiceStatus};
use crate::utils::messages::services::module::service::{
    errors as module_service_errors, logs as module_service_logs, notes as module_service_notes,
};
use tokio::sync::RwLock;

use super::dev::{DeclaredServicesState, DevOverrideState, DevSourceConfig};
use super::types::{DistributionAction, DistributionPlanEntry, ModuleUpdateInfo};

#[derive(Clone)]
pub struct ModuleService {
    pub(super) module_registry: Arc<dyn ModuleRegistryPort>,
    pub(super) storage: Arc<dyn ModuleStoragePort>,
    pub(super) verifier: Arc<dyn ModuleVerifierPort>,
    pub(super) runtime: Arc<dyn ModuleRuntimePort>,
    pub(super) service_registry: Arc<ServiceRegistry>,
    pub(super) dev_sources: Option<DevSourceConfig>,
    pub(super) dev_overrides: Arc<RwLock<HashMap<ModuleId, DevOverrideState>>>,
    pub(super) declared_services: Arc<RwLock<HashMap<ModuleId, DeclaredServicesState>>>,
}

impl ModuleService {
    pub fn new(
        registry: Arc<dyn ModuleRegistryPort>,
        storage: Arc<dyn ModuleStoragePort>,
        verifier: Arc<dyn ModuleVerifierPort>,
        runtime: Arc<dyn ModuleRuntimePort>,
        service_registry: Arc<ServiceRegistry>,
        dev_sources: Option<PathBuf>,
    ) -> Self {
        Self {
            module_registry: registry,
            storage,
            verifier,
            runtime,
            service_registry,
            dev_sources: dev_sources.map(DevSourceConfig::new),
            dev_overrides: Arc::new(RwLock::new(HashMap::new())),
            declared_services: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[allow(dead_code)]
    fn module_service_id(module_id: &ModuleId) -> String {
        format!("module:{}", module_id)
    }

    #[allow(dead_code)]
    fn module_service_descriptor(
        module_id: &ModuleId,
        manifest: &ModuleManifest,
    ) -> ServiceDescriptorOwned {
        let _ = (module_id, manifest);
        ServiceDescriptorOwned::new(
            String::new(),
            String::new(),
            String::new(),
            ServiceKind::Other,
        )
    }

    fn ensure_module_service_entry(&self, module_id: &ModuleId, manifest: &ModuleManifest) {
        let _ = (module_id, manifest);
    }

    pub(super) fn update_module_service_status(
        &self,
        module_id: &ModuleId,
        manifest: &ModuleManifest,
        status: ServiceStatus,
        note: impl Into<Option<String>>,
    ) {
        let _ = (module_id, manifest, status, note.into());
    }

    fn unregister_module_service(&self, module_id: &ModuleId) {
        let _ = module_id;
    }

    pub(super) fn is_unmanaged_module(&self, module_id: &ModuleId) -> bool {
        let _ = module_id;
        false
    }

    pub(super) async fn is_dev_override_active(&self, module_id: &ModuleId) -> bool {
        self.dev_overrides.read().await.contains_key(module_id)
    }

    /// Search for modules in the registry
    pub async fn search(&self, query: ModuleSearchQuery) -> ModuleResult<Vec<ModuleSummary>> {
        let summaries = self.module_registry.search(query).await?;
        Ok(summaries)
    }

    /// List all installed modules
    pub async fn list_installed(&self) -> ModuleResult<Vec<InstalledModule>> {
        let modules = self.storage.list().await?;
        for module in &modules {
            if let Ok(module_id) = module.manifest.module_id() {
                self.ensure_module_service_entry(&module_id, &module.manifest);
            }
        }
        Ok(modules)
    }

    /// Install or update a module
    pub async fn install(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> ModuleResult<ModuleInstallResult> {
        self.install_with_progress(id, version, None).await
    }

    /// Install or update a module with progress callback
    pub async fn install_with_progress(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
        progress: Option<ProgressCallback>,
    ) -> ModuleResult<ModuleInstallResult> {
        let manifest = self.module_registry.fetch_manifest(id, version).await?;
        self.clear_dev_services_if_any(id).await;
        self.clear_declared_services_if_any(id).await;

        // Check if already installed
        if let Some(installed) = self.storage.load(id).await? {
            if installed.manifest.version == manifest.version {
                if let Err(err) = self.register_declared_services(id, &installed).await {
                    tracing::warn!(
                        module = %installed.manifest.id,
                        error = %err,
                        "{}",
                        module_service_logs::DECLARED_SERVICES_REFRESH_FAILED
                    );
                }
                return Ok(ModuleInstallResult {
                    status: ModuleInstallStatus::AlreadyCurrent,
                    manifest: installed.manifest,
                    path: installed.path,
                    source: installed.source,
                });
            }
            // Stop running instance before updating
            self.stop_module_process(id).await;
        }

        let bundle = self
            .module_registry
            .download_with_progress(&manifest, progress)
            .await?;
        self.verifier.verify(&bundle).await?;
        let result = self
            .storage
            .stage_and_activate(bundle, ModuleInstallSource::Distribution)
            .await?;

        if let Ok(module_id) = ModuleId::new(&result.manifest.id) {
            self.update_module_service_status(
                &module_id,
                &result.manifest,
                ServiceStatus::Standby,
                Some(module_service_notes::INSTALLED.to_string()),
            );
            if !self.is_dev_override_active(&module_id).await {
                if let Err(err) = self.ensure_running(&module_id).await {
                    tracing::warn!(
                        module = %module_id,
                        error = %err,
                        "{}",
                        module_service_logs::AUTO_START_FAILED
                    );
                    self.update_module_service_status(
                        &module_id,
                        &result.manifest,
                        ServiceStatus::Degraded,
                        Some(module_service_notes::start_failed(&err)),
                    );
                }
            }
        }

        Ok(result)
    }

    /// Get manifest from registry (without installing)
    pub async fn manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> ModuleResult<ModuleManifest> {
        let manifest = self.module_registry.fetch_manifest(id, version).await?;
        Ok(manifest)
    }

    /// Get installed module info
    pub async fn installed(&self, id: &ModuleId) -> ModuleResult<Option<InstalledModule>> {
        let installed = self.storage.load(id).await?;
        Ok(installed)
    }

    /// Uninstall a module from local storage
    pub async fn uninstall(&self, id: &ModuleId) -> ModuleResult<()> {
        if self.storage.load(id).await?.is_none() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_service_errors::module_not_installed(id)),
            ));
        }
        self.storage.remove(id).await?;
        self.clear_dev_services_if_any(id).await;
        self.clear_declared_services_if_any(id).await;
        self.unregister_module_service(id);
        Ok(())
    }

    /// Check for available updates for all installed modules
    pub async fn check_updates(
        &self,
        fenrir_version: Option<&str>,
    ) -> ModuleResult<Vec<ModuleUpdateInfo>> {
        let installed = self.list_installed().await?;
        let mut updates = Vec::new();

        for module in installed {
            let module_id = module.manifest.module_id().map_err(|e| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(e.to_string()))
            })?;

            // Fetch latest version from registry
            match self.module_registry.fetch_manifest(&module_id, None).await {
                Ok(latest_manifest) => {
                    let current_version = ModuleVersion(module.manifest.version.clone());
                    let latest_version = ModuleVersion(latest_manifest.version.clone());

                    let has_update = latest_version.0 > current_version.0;

                    // Check Fenrir compatibility
                    let compatible = if let Some(fenrir_ver) = fenrir_version {
                        check_fenrir_compatibility(&latest_manifest, fenrir_ver)
                    } else {
                        true // Assume compatible if no version provided
                    };

                    updates.push(ModuleUpdateInfo {
                        module_id: module_id.clone(),
                        current_version,
                        latest_version,
                        has_update,
                        compatible,
                    });
                }
                Err(_) => {
                    // If we can't fetch the module (maybe removed from registry),
                    // still add it but mark as no update
                    let current_version = ModuleVersion(module.manifest.version.clone());
                    updates.push(ModuleUpdateInfo {
                        module_id: module_id.clone(),
                        current_version: current_version.clone(),
                        latest_version: current_version,
                        has_update: false,
                        compatible: true,
                    });
                }
            }
        }

        Ok(updates)
    }

    /// Build a plan for installing/updating all modules compatible with the target Fenrir version
    pub async fn distribution_plan(
        &self,
        fenrir_version: &str,
    ) -> ModuleResult<Vec<DistributionPlanEntry>> {
        let targets = self
            .module_registry
            .distribution_targets(fenrir_version)
            .await?;
        let installed = self.list_installed().await?;
        let mut current_map = HashMap::new();
        for module in installed {
            let module_id = module.manifest.module_id().map_err(|e| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(e.to_string()))
            })?;
            current_map.insert(module_id, module.manifest.module_version());
        }

        let mut plan = Vec::new();
        for DistributionTarget { module_id, version } in targets {
            let current = current_map.get(&module_id).cloned();
            let action = match current.as_ref() {
                None => DistributionAction::Install,
                Some(current_version) if current_version < &version => DistributionAction::Update,
                _ => DistributionAction::AlreadyCurrent,
            };

            plan.push(DistributionPlanEntry {
                module_id,
                target_version: version,
                current_version: current,
                action,
            });
        }

        plan.sort_by(|a, b| a.module_id.cmp(&b.module_id));
        Ok(plan)
    }

    /// Apply a distribution plan by installing/updating required modules
    pub async fn apply_distribution_plan(
        &self,
        plan: Vec<DistributionPlanEntry>,
    ) -> ModuleResult<Vec<ModuleInstallResult>> {
        let mut results = Vec::new();
        for entry in plan {
            if !entry.action.requires_execution() {
                continue;
            }

            let result = self
                .install(&entry.module_id, Some(&entry.target_version))
                .await?;

            if matches!(result.status, ModuleInstallStatus::AlreadyCurrent) {
                continue;
            }

            results.push(result);
        }

        Ok(results)
    }

    /// Release a locally synchronized module back to its distribution artifact.
    pub async fn release_override(
        &self,
        module_id: &ModuleId,
        fenrir_version: &str,
    ) -> ModuleResult<ModuleInstallResult> {
        let installed = self.storage.load(module_id).await?.ok_or_else(|| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(
                module_service_errors::module_missing(module_id),
            ))
        })?;

        let dev_override_active = self.clear_dev_services_if_any(module_id).await;

        if !installed.source.is_synchronized() && !dev_override_active {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_service_errors::already_on_distribution(
                    module_id,
                )),
            ));
        }

        let targets = self
            .module_registry
            .distribution_targets(fenrir_version)
            .await?;
        let target_version = targets
            .into_iter()
            .find(|entry| &entry.module_id == module_id)
            .map(|entry| entry.version)
            .ok_or_else(|| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(
                    module_service_errors::not_part_of_distribution(module_id, fenrir_version),
                ))
            })?;

        let result = self.install(module_id, Some(&target_version)).await?;
        self.ensure_all_running().await;
        Ok(result)
    }

    /// Update a specific module to latest compatible version
    pub async fn update(
        &self,
        id: &ModuleId,
        fenrir_version: Option<&str>,
    ) -> ModuleResult<ModuleInstallResult> {
        // Fetch latest version
        let manifest = self.module_registry.fetch_manifest(id, None).await?;

        // Check Fenrir compatibility
        if let Some(fenrir_ver) = fenrir_version {
            if !check_fenrir_compatibility(&manifest, fenrir_ver) {
                return Err(ModuleServiceError::Storage(
                    ModuleStorageError::InvalidState(module_service_errors::incompatible_version(
                        id,
                        &manifest.version,
                        fenrir_ver,
                    )),
                ));
            }
        }

        // Install the update
        self.install(id, None).await
    }

    /// Update all modules
    pub async fn update_all(
        &self,
        fenrir_version: Option<&str>,
    ) -> ModuleResult<Vec<ModuleInstallResult>> {
        let updates = self.check_updates(fenrir_version).await?;
        let mut results = Vec::new();

        for update_info in updates {
            if update_info.has_update && update_info.compatible {
                match self.update(&update_info.module_id, fenrir_version).await {
                    Ok(result) => results.push(result),
                    Err(e) => {
                        tracing::warn!(
                            module = %update_info.module_id,
                            error = %e,
                            "{}",
                            module_service_logs::UPDATE_FAILED
                        );
                    }
                }
            }
        }

        Ok(results)
    }
}

/// Check if a module is compatible with a specific Fenrir version
fn check_fenrir_compatibility(manifest: &ModuleManifest, fenrir_version: &str) -> bool {
    if let Some(ref req) = manifest.fenrir_version {
        // Parse the Fenrir version
        if let Ok(version) = semver::Version::parse(fenrir_version) {
            return req.matches(&version);
        }
    }

    // If no requirement specified, assume compatible
    true
}
