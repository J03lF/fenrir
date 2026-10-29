//! What the depot pipeline needs from the outside world: the depot itself
//! (`DepotSource`, implemented over mTLS by `infra::modules::depot`) and the
//! host's module runtime (`ModuleHost`, implemented by `host::ModuleServiceHost`
//! on top of `ModuleService`). Tests replace both.

use std::path::Path;

use async_trait::async_trait;
use depot_protocol::api::{DistributionDetail, DistributionSummary, ModuleInfo, SearchResponse};
use depot_protocol::ModuleVersion as DepotVersion;
use depot_protocol::{DistributionId, Name, Sha256Digest};

use crate::domain::module::{ModuleBundle, ModuleManifest};

use super::blob::BlobWriter;
use super::error::DepotError;

/// Read access to a depot (the `reader` role is enough).
#[async_trait]
pub trait DepotSource: Send + Sync {
    /// `GET /v1/distributions/{id}`; `None` on 404.
    async fn distribution(
        &self,
        id: &DistributionId,
    ) -> Result<Option<DistributionDetail>, DepotError>;
    /// `GET /v1/distributions?product=`, newest first (unsigned summaries:
    /// only a list of candidates, every one is verified before use).
    async fn distributions(&self, product: &Name) -> Result<Vec<DistributionSummary>, DepotError>;
    /// `GET /v1/modules/{name}`; `None` on 404.
    async fn module(&self, name: &Name) -> Result<Option<ModuleInfo>, DepotError>;
    /// `GET /v1/search?q=`.
    async fn search(&self, query: &str) -> Result<SearchResponse, DepotError>;
    /// `GET /v1/blobs/{sha256}`, every chunk into `sink`, which checks size
    /// and SHA-256 while it writes.
    async fn blob(
        &self,
        digest: &Sha256Digest,
        size: u64,
        sink: &mut BlobWriter,
    ) -> Result<(), DepotError>;
}

/// An installed module as the pipeline sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostModule {
    pub name: String,
    pub version: DepotVersion,
    /// SHA-256 recorded at install (the bundle's digest for depot installs).
    pub sha256: String,
    /// Synchronized from local sources (`synchronize module`) or a dev
    /// override: the depot never touches it.
    pub local_sync: bool,
    pub running: bool,
    /// Runtime instances; above one a rolling restart replaces them one by
    /// one under health checks.
    pub instances: usize,
    /// `*.sql` files of the installed module's migrations directory.
    pub migrations: Vec<String>,
}

/// The module runtime of this host.
#[async_trait]
pub trait ModuleHost: Send + Sync {
    async fn modules(&self) -> Result<Vec<HostModule>, String>;
    /// Writes a gzip tar of the installed module to `archive` and returns
    /// its manifest; `None` when the module is not installed.
    async fn backup(&self, module: &str, archive: &Path) -> Result<Option<ModuleManifest>, String>;
    async fn stop(&self, module: &str) -> Result<(), String>;
    /// Replaces the module's files with the (verified) bundle; a lower
    /// version than the installed one is allowed. Does not start it.
    async fn activate(&self, bundle: ModuleBundle) -> Result<(), String>;
    /// Uninstalls (rollback of a module that was not there before).
    async fn remove(&self, module: &str) -> Result<(), String>;
    /// Applies pending database migrations of these modules (the runner the
    /// host uses at boot). Returns the applied files.
    async fn apply_migrations(&self, modules: &[String]) -> Result<Vec<String>, String>;
    /// Starts every installed module that is not running, in dependency
    /// order, and fails if one of `modules` does not run afterwards.
    async fn start(&self, modules: &[String]) -> Result<(), String>;
    /// Health-gated replacement of a running module's instances.
    async fn rolling_restart(&self, module: &str) -> Result<(), String>;
    /// Waits for the module's health check.
    async fn wait_healthy(&self, module: &str) -> Result<(), String>;
}
