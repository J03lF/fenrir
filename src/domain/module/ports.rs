use async_trait::async_trait;

use super::bundle::ModuleBundle;
use super::errors::{ModuleRegistryError, ModuleStorageError, ModuleVerificationError};
use super::id::ModuleId;
use super::install::{
    DistributionTarget, InstalledModule, ModuleInstallResult, ModuleInstallSource,
};
use super::manifest::ModuleManifest;
use super::progress::ProgressCallback;
use super::search::ModuleSearchQuery;
use super::summary::ModuleSummary;
use super::version::ModuleVersion;

#[async_trait]
pub trait ModuleRegistryPort: Send + Sync {
    async fn search(
        &self,
        query: ModuleSearchQuery,
    ) -> Result<Vec<ModuleSummary>, ModuleRegistryError>;
    async fn fetch_manifest(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
    ) -> Result<ModuleManifest, ModuleRegistryError>;
    async fn download(
        &self,
        manifest: &ModuleManifest,
    ) -> Result<ModuleBundle, ModuleRegistryError>;
    async fn download_with_progress(
        &self,
        manifest: &ModuleManifest,
        progress: Option<ProgressCallback>,
    ) -> Result<ModuleBundle, ModuleRegistryError>;
    async fn distribution_targets(
        &self,
        fenrir_version: &str,
    ) -> Result<Vec<DistributionTarget>, ModuleRegistryError>;
}

#[async_trait]
pub trait ModuleStoragePort: Send + Sync {
    async fn list(&self) -> Result<Vec<InstalledModule>, ModuleStorageError>;
    async fn load(&self, id: &ModuleId) -> Result<Option<InstalledModule>, ModuleStorageError>;
    async fn stage_and_activate(
        &self,
        bundle: ModuleBundle,
        source: ModuleInstallSource,
    ) -> Result<ModuleInstallResult, ModuleStorageError>;
    async fn remove(&self, id: &ModuleId) -> Result<(), ModuleStorageError>;
}

#[async_trait]
pub trait ModuleVerifierPort: Send + Sync {
    async fn verify(&self, bundle: &ModuleBundle) -> Result<(), ModuleVerificationError>;
}
