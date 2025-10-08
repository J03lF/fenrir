use std::sync::Arc;

use crate::domain::module::{
    InstalledModule, ModuleId, ModuleInstallResult, ModuleInstallStatus, ModuleManifest,
    ModuleRegistryPort, ModuleResult, ModuleSearchQuery, ModuleStoragePort, ModuleSummary,
    ModuleVerifierPort, ModuleVersion,
};

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

    pub async fn search(&self, query: ModuleSearchQuery) -> ModuleResult<Vec<ModuleSummary>> {
        let summaries = self.registry.search(query).await?;
        Ok(summaries)
    }

    pub async fn list_installed(&self) -> ModuleResult<Vec<InstalledModule>> {
        Ok(self.storage.list().await?)
    }

    pub async fn install(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> ModuleResult<ModuleInstallResult> {
        let manifest = self.registry.fetch_manifest(id, version).await?;
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

    pub async fn manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> ModuleResult<ModuleManifest> {
        let manifest = self.registry.fetch_manifest(id, version).await?;
        Ok(manifest)
    }

    pub async fn installed(&self, id: &ModuleId) -> ModuleResult<Option<InstalledModule>> {
        let installed = self.storage.load(id).await?;
        Ok(installed)
    }
}
