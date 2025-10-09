use std::sync::Arc;

use crate::domain::module::{
    InstalledModule, ModuleId, ModuleInstallResult, ModuleInstallStatus, ModuleManifest,
    ModuleRegistryPort, ModuleResult, ModuleSearchQuery, ModuleServiceError, ModuleStorageError,
    ModuleStoragePort, ModuleSummary, ModuleVerifierPort, ModuleVersion,
};

/// Information about available updates for a module
#[derive(Debug, Clone)]
pub struct ModuleUpdateInfo {
    pub module_id: ModuleId,
    pub current_version: ModuleVersion,
    pub latest_version: ModuleVersion,
    pub has_update: bool,
    pub compatible: bool,
}

#[derive(Clone)]
pub struct ModuleService {
    registry: Arc<dyn ModuleRegistryPort>,
    storage: Arc<dyn ModuleStoragePort>,
    verifier: Arc<dyn ModuleVerifierPort>,
}

impl ModuleService {
    pub fn new(
        registry: Arc<dyn ModuleRegistryPort>,
        storage: Arc<dyn ModuleStoragePort>,
        verifier: Arc<dyn ModuleVerifierPort>,
    ) -> Self {
        Self {
            registry,
            storage,
            verifier,
        }
    }

    /// Search for modules in the registry
    pub async fn search(&self, query: ModuleSearchQuery) -> ModuleResult<Vec<ModuleSummary>> {
        let summaries = self.registry.search(query).await?;
        Ok(summaries)
    }

    /// List all installed modules
    pub async fn list_installed(&self) -> ModuleResult<Vec<InstalledModule>> {
        Ok(self.storage.list().await?)
    }

    /// Install or update a module
    pub async fn install(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> ModuleResult<ModuleInstallResult> {
        let manifest = self.registry.fetch_manifest(id, version).await?;

        // Check if already installed
        if let Some(installed) = self.storage.load(id).await? {
            if installed.manifest.version == manifest.version {
                return Ok(ModuleInstallResult {
                    status: ModuleInstallStatus::AlreadyCurrent,
                    manifest: installed.manifest,
                    path: installed.path,
                });
            }
        }

        let bundle = self.registry.download(&manifest).await?;
        self.verifier.verify(&bundle).await?;
        let result = self.storage.stage_and_activate(bundle).await?;
        Ok(result)
    }

    /// Get manifest from registry (without installing)
    pub async fn manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> ModuleResult<ModuleManifest> {
        let manifest = self.registry.fetch_manifest(id, version).await?;
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
            return Err(ModuleServiceError::Storage(ModuleStorageError::InvalidState(
                format!("Module {} is not installed", id),
            )));
        }
        self.storage.remove(id).await?;
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
            match self.registry.fetch_manifest(&module_id, None).await {
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

    /// Update a specific module to latest compatible version
    pub async fn update(
        &self,
        id: &ModuleId,
        fenrir_version: Option<&str>,
    ) -> ModuleResult<ModuleInstallResult> {
        // Fetch latest version
        let manifest = self.registry.fetch_manifest(id, None).await?;

        // Check Fenrir compatibility
        if let Some(fenrir_ver) = fenrir_version {
            if !check_fenrir_compatibility(&manifest, fenrir_ver) {
                return Err(ModuleServiceError::Storage(
                    ModuleStorageError::InvalidState(format!(
                        "Module {} v{} is not compatible with Fenrir {}",
                        id, manifest.version, fenrir_ver
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
                        // Log error but continue with other updates
                        tracing::warn!("Failed to update module {}: {}", update_info.module_id, e);
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
