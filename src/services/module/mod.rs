use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::domain::module::{
    ChecksumAlgorithm, DistributionTarget, InstalledModule, ModuleBundle, ModuleId,
    ModuleInstallResult, ModuleInstallSource, ModuleInstallStatus, ModuleManifest,
    ModuleRegistryPort, ModuleResult, ModuleRuntimeError, ModuleRuntimeInfo, ModuleRuntimePort,
    ModuleRuntimeStatus, ModuleSearchQuery, ModuleServiceError, ModuleStartConfig,
    ModuleStorageError, ModuleStoragePort, ModuleSummary, ModuleVerifierPort, ModuleVersion,
    ProgressCallback,
};
use crate::services::{
    ServiceDescriptorOwned, ServiceKind, ServiceRegistry, ServiceStatus, ServiceTag,
};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tar::Builder;
use tokio::sync::RwLock;
use tokio::task;

/// Information about available updates for a module
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
            DistributionAction::Install => "Install",
            DistributionAction::Update => "Update",
            DistributionAction::AlreadyCurrent => "Aktuell",
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

#[derive(Clone)]
pub struct ModuleService {
    module_registry: Arc<dyn ModuleRegistryPort>,
    storage: Arc<dyn ModuleStoragePort>,
    verifier: Arc<dyn ModuleVerifierPort>,
    runtime: Arc<dyn ModuleRuntimePort>,
    service_registry: Arc<ServiceRegistry>,
    dev_sources: Option<DevSourceConfig>,
    dev_overrides: Arc<RwLock<HashMap<ModuleId, DevOverrideState>>>,
    declared_services: Arc<RwLock<HashMap<ModuleId, DeclaredServicesState>>>,
}

#[derive(Clone)]
struct DevSourceConfig {
    base_path: PathBuf,
}

impl DevSourceConfig {
    fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    fn module_root(&self, module_id: &ModuleId) -> PathBuf {
        self.base_path.join(module_id.as_str())
    }
}

#[derive(Debug, Clone)]
struct DevSourceInfo {
    package_source: Option<PathBuf>,
    services: Vec<DevServiceBinding>,
}

#[derive(Debug, Clone)]
struct DevServiceBinding {
    id: String,
    endpoint: SocketAddr,
    name: Option<String>,
    description: Option<String>,
    kind: ServiceKind,
}

#[derive(Debug, Default)]
struct DevOverrideState {
    service_ids: Vec<String>,
}

#[derive(Debug, Default)]
struct DeclaredServicesState {
    service_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ModuleSyncOutcome {
    Packaged(ModuleSyncPackage),
    ExternalServices(ModuleDevServices),
}

#[derive(Debug, Clone)]
pub struct ModuleSyncPackage {
    pub install_result: ModuleInstallResult,
    pub packaged_from: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ModuleDevServices {
    pub module_id: ModuleId,
    pub version: ModuleVersion,
    pub services: Vec<RegisteredDevService>,
}

#[derive(Debug, Clone)]
pub struct RegisteredDevService {
    pub service_id: String,
    pub endpoint: SocketAddr,
    pub name: String,
    pub description: Option<String>,
    pub kind: ServiceKind,
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

    fn update_module_service_status(
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

    fn is_unmanaged_module(&self, module_id: &ModuleId) -> bool {
        let _ = module_id;
        false
    }

    async fn is_dev_override_active(&self, module_id: &ModuleId) -> bool {
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
                        "failed to refresh declared services for already installed module"
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

        let bundle = self.module_registry.download_with_progress(&manifest, progress).await?;
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
                Some("installiert".to_string()),
            );
            if !self.is_dev_override_active(&module_id).await {
                if let Err(err) = self.ensure_running(&module_id).await {
                    tracing::warn!(module = %module_id, error = %err, "failed to auto-start module");
                    self.update_module_service_status(
                        &module_id,
                        &result.manifest,
                        ServiceStatus::Degraded,
                        Some(format!("Start fehlgeschlagen: {err}")),
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
                ModuleStorageError::InvalidState(format!("Module {} is not installed", id)),
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

    /// Synchronize an installed module with the files located on this machine.
    pub async fn synchronize_from_local(
        &self,
        module_id: &ModuleId,
    ) -> ModuleResult<ModuleSyncOutcome> {
        let installed = self.storage.load(module_id).await?.ok_or_else(|| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(format!(
                "Module {} ist nicht installiert",
                module_id
            )))
        })?;

        if let Err(err) = self.stop_all_modules().await {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Module konnten nicht gestoppt werden: {err}"
                )),
            ));
        }

        if let Some(dev_info) = self.locate_dev_source(module_id)? {
            if !dev_info.services.is_empty() {
                tracing::info!(
                    module = %module_id,
                    services = dev_info.services.len(),
                    "activating module dev service override"
                );
                return self
                    .activate_dev_services(module_id, &installed, dev_info.services)
                    .await;
            }

            if let Some(source) = dev_info.package_source {
                tracing::info!(
                    module = %module_id,
                    path = %source.display(),
                    "packaging module from dev sources"
                );
                return self
                    .install_from_directory(module_id, &installed, source)
                    .await;
            }
        }

        let module_dir = PathBuf::from(&installed.path);
        if !module_dir.exists() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Installationspfad {} existiert nicht",
                    module_dir.display()
                )),
            ));
        }

        self.install_from_directory(module_id, &installed, module_dir)
            .await
    }

    async fn install_from_directory(
        &self,
        module_id: &ModuleId,
        installed: &InstalledModule,
        package_source: PathBuf,
    ) -> ModuleResult<ModuleSyncOutcome> {
        self.clear_dev_services_if_any(module_id).await;
        let archive = package_module_directory(package_source.clone()).await?;
        let mut hasher = Sha256::new();
        hasher.update(&archive);
        let checksum = hasher.finalize().to_vec();

        let mut manifest = installed.manifest.clone();
        manifest.artifact.download_url = format!(
            "file://local/{}/{}-local.tar.gz",
            manifest.id, manifest.version
        );
        manifest.artifact.checksum.hash = hex::encode(&checksum);
        manifest.artifact.checksum.algorithm = ChecksumAlgorithm::Sha256;
        manifest.artifact.content_type = Some("application/gzip".to_string());
        manifest.artifact.size_bytes = Some(archive.len() as u64);

        let bundle = ModuleBundle {
            manifest,
            archive,
            signature: Vec::new(),
            checksum,
        };

        let result = self
            .storage
            .stage_and_activate(bundle, ModuleInstallSource::LocalOverride)
            .await?;

        if let Ok(Some(updated_installation)) = self.storage.load(module_id).await {
            if let Err(err) = self
                .register_declared_services(module_id, &updated_installation)
                .await
            {
                tracing::warn!(
                    module = %module_id,
                    error = %err,
                    "failed to register declared services after sync"
                );
            }
        }

        Ok(ModuleSyncOutcome::Packaged(ModuleSyncPackage {
            install_result: result,
            packaged_from: package_source,
        }))
    }

    async fn activate_dev_services(
        &self,
        module_id: &ModuleId,
        installed: &InstalledModule,
        bindings: Vec<DevServiceBinding>,
    ) -> ModuleResult<ModuleSyncOutcome> {
        self.clear_dev_services_if_any(module_id).await;
        self.clear_declared_services_if_any(module_id).await;
        if let Err(err) = self.runtime.stop(module_id).await {
            if !matches!(err, ModuleRuntimeError::NotRunning { .. }) {
                tracing::warn!(
                    module = %module_id,
                    error = %err,
                    "failed to stop module runtime before activating dev services"
                );
            }
        }

        if bindings.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Keine Dev-Services für Modul {} konfiguriert",
                    module_id
                )),
            ));
        }

        let mut registered = Vec::new();
        let mut ids = Vec::new();

        for binding in bindings {
            let service_id = format!("module:{}::{}", module_id, binding.id);
            let service_name = binding
                .name
                .clone()
                .unwrap_or_else(|| format!("{} ({})", module_id, binding.id));
            let service_description = binding
                .description
                .clone()
                .unwrap_or_else(|| "Dev-Service".to_string());
            let descriptor = ServiceDescriptorOwned::new(
                service_id.clone(),
                service_name.clone(),
                service_description,
                binding.kind,
            )
            .with_tags(vec![ServiceTag::Auxiliary]);

            self.service_registry.register(
                descriptor,
                ServiceStatus::Active,
                Some(format!("dev endpoint {}", binding.endpoint)),
            );

            registered.push(RegisteredDevService {
                service_id: service_id.clone(),
                endpoint: binding.endpoint,
                name: service_name,
                description: binding.description.clone(),
                kind: binding.kind,
            });
            ids.push(service_id);
        }

        self.remember_dev_services(module_id, ids).await;
        self.update_module_service_status(
            module_id,
            &installed.manifest,
            ServiceStatus::Standby,
            Some("Dev-Service override aktiv".to_string()),
        );

        Ok(ModuleSyncOutcome::ExternalServices(ModuleDevServices {
            module_id: module_id.clone(),
            version: installed.manifest.module_version(),
            services: registered,
        }))
    }

    async fn register_declared_services(
        &self,
        module_id: &ModuleId,
        installed: &InstalledModule,
    ) -> ModuleResult<()> {
        if self.is_dev_override_active(module_id).await {
            return Ok(());
        }

        let module_root = PathBuf::from(&installed.path);
        let declared = load_declared_services(&module_root)?;
        self.clear_declared_services_if_any(module_id).await;

        if declared.is_empty() {
            return Ok(());
        }

        let mut ids = Vec::new();
        for binding in declared {
            let service_id = binding.id.clone();
            let name = binding.name.clone().unwrap_or_else(|| binding.id.clone());
            let description = binding
                .description
                .clone()
                .unwrap_or_else(|| format!("Service {} aus Modul {}", name, module_id));
            let descriptor =
                ServiceDescriptorOwned::new(service_id.clone(), name, description, binding.kind)
                    .with_tags(vec![ServiceTag::Auxiliary]);
            self.service_registry.register(
                descriptor,
                ServiceStatus::Active,
                Some(format!("endpoint {}", binding.endpoint)),
            );
            ids.push(service_id);
        }

        self.remember_declared_services(module_id, ids).await;
        Ok(())
    }

    /// Release a locally synchronized module back to its distribution artifact.
    pub async fn release_override(
        &self,
        module_id: &ModuleId,
        fenrir_version: &str,
    ) -> ModuleResult<ModuleInstallResult> {
        let installed = self.storage.load(module_id).await?.ok_or_else(|| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(format!(
                "Module {} ist nicht installiert",
                module_id
            )))
        })?;

        let dev_override_active = self.clear_dev_services_if_any(module_id).await;

        if !installed.source.is_synchronized() && !dev_override_active {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Module {} nutzt bereits die Distribution",
                    module_id
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
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(format!(
                    "Module {} ist nicht Teil der Distribution für Fenrir {}",
                    module_id, fenrir_version
                )))
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

    // ========================================================================
    // Module Runtime Management
    // ========================================================================

    pub async fn ensure_all_running(&self) {
        let modules = match self.list_installed().await {
            Ok(list) => list,
            Err(err) => {
                tracing::warn!(error = %err, "failed to list modules for auto-start");
                return;
            }
        };

        for module in modules {
            let Ok(module_id) = module.manifest.module_id() else {
                continue;
            };
            if let Err(err) = self.ensure_running(&module_id).await {
                tracing::warn!(module = %module_id, error = %err, "auto-start failed");
            }
        }
    }

    pub async fn stop_all_modules(&self) -> Result<(), ModuleRuntimeError> {
        let running = self.runtime.list_running().await?;
        for info in running {
            if let Err(err) = self.runtime.stop(&info.module_id).await {
                if !matches!(err, ModuleRuntimeError::NotRunning { .. }) {
                    tracing::warn!(
                        module = %info.module_id,
                        error = %err,
                        "failed to stop module during sync"
                    );
                }
            }
        }
        Ok(())
    }

    async fn stop_module_process(&self, module_id: &ModuleId) {
        match self.runtime.stop(module_id).await {
            Ok(_) => {}
            Err(ModuleRuntimeError::NotRunning { .. }) => {}
            Err(err) => {
                tracing::warn!(
                    module = %module_id,
                    error = %err,
                    "failed to stop module before update"
                );
            }
        }
    }

    pub async fn ensure_running(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
        let installed = match self.storage.load(module_id).await {
            Ok(Some(installed)) => installed,
            Ok(None) => return Ok(()),
            Err(err) => {
                return Err(ModuleRuntimeError::InvalidState(err.to_string()));
            }
        };

        if self.is_unmanaged_module(module_id) {
            return Ok(());
        }

        self.register_declared_services(module_id, &installed)
            .await
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?;

        if self.is_dev_override_active(module_id).await {
            return Ok(());
        }

        match self.runtime.status(module_id).await {
            Ok(info) if matches!(info.status, ModuleRuntimeStatus::Running) => {
                self.update_module_service_status(
                    module_id,
                    &installed.manifest,
                    ServiceStatus::Active,
                    Some(
                        info.pid
                            .map(|pid| format!("läuft (PID {pid})"))
                            .unwrap_or_else(|| "läuft".to_string()),
                    ),
                );
                Ok(())
            }
            Ok(_) | Err(ModuleRuntimeError::NotRunning { .. }) => {
                let config = ModuleStartConfig {
                    module_id: module_id.clone(),
                    port: None,
                    env_vars: Vec::new(),
                    auto_restart: true,
                };
                self.start(config).await.map(|_| ())
            }
            Err(err) => Err(err),
        }
    }

    /// Start a module instance
    pub async fn start(
        &self,
        config: ModuleStartConfig,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        // Verify module is installed
        let installed = self
            .storage
            .load(&config.module_id)
            .await
            .map_err(|e| ModuleRuntimeError::InvalidState(e.to_string()))?
            .ok_or_else(|| ModuleRuntimeError::NotInstalled {
                module_id: config.module_id.to_string(),
            })?;

        if self.is_unmanaged_module(&config.module_id) {
            return Ok(ModuleRuntimeInfo {
                module_id: config.module_id.clone(),
                version: ModuleVersion(installed.manifest.version.clone()),
                status: ModuleRuntimeStatus::Stopped,
                pid: None,
                port: config.port,
                started_at: None,
                stopped_at: Some(SystemTime::now()),
                restart_count: 0,
            });
        }

        self.register_declared_services(&config.module_id, &installed)
            .await
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))?;

        if self.is_dev_override_active(&config.module_id).await {
            return Ok(ModuleRuntimeInfo {
                module_id: config.module_id.clone(),
                version: ModuleVersion(installed.manifest.version.clone()),
                status: ModuleRuntimeStatus::Running,
                pid: None,
                port: config.port,
                started_at: Some(SystemTime::now()),
                stopped_at: None,
                restart_count: 0,
            });
        }

        // Check if already running
        if let Ok(info) = self.runtime.status(&config.module_id).await {
            if matches!(
                info.status,
                crate::domain::module::ModuleRuntimeStatus::Running
            ) {
                return Err(ModuleRuntimeError::AlreadyRunning {
                    module_id: config.module_id.to_string(),
                });
            }
        }

        let manifest = installed.manifest.clone();
        // Start the module
        let runtime_info = self.runtime.start(config).await?;

        tracing::info!(
            module_id = %runtime_info.module_id,
            pid = ?runtime_info.pid,
            port = ?runtime_info.port,
            "module started successfully"
        );

        self.update_module_service_status(
            &runtime_info.module_id,
            &manifest,
            ServiceStatus::Active,
            Some(
                runtime_info
                    .pid
                    .map(|pid| format!("läuft (PID {pid})"))
                    .unwrap_or_else(|| "läuft".to_string()),
            ),
        );

        Ok(runtime_info)
    }

    /// Stop a running module instance
    pub async fn stop(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
        if self.is_unmanaged_module(module_id) || self.is_dev_override_active(module_id).await {
            return Ok(());
        }

        self.runtime.stop(module_id).await?;

        tracing::info!(
            module_id = %module_id,
            "module stopped successfully"
        );

        if let Some(installed) = self
            .storage
            .load(module_id)
            .await
            .map_err(|e| ModuleRuntimeError::InvalidState(e.to_string()))?
        {
            self.update_module_service_status(
                module_id,
                &installed.manifest,
                ServiceStatus::Stopped,
                Some("gestoppt".to_string()),
            );
        }

        Ok(())
    }

    /// Get runtime status of a module
    pub async fn runtime_status(
        &self,
        module_id: &ModuleId,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        self.runtime.status(module_id).await
    }

    /// List all running modules
    pub async fn list_running(&self) -> Result<Vec<ModuleRuntimeInfo>, ModuleRuntimeError> {
        self.runtime.list_running().await
    }

    /// Restart a module instance
    pub async fn restart(
        &self,
        module_id: &ModuleId,
    ) -> Result<ModuleRuntimeInfo, ModuleRuntimeError> {
        tracing::info!(module_id = %module_id, "restarting module");
        let installed = self
            .storage
            .load(module_id)
            .await
            .map_err(|e| ModuleRuntimeError::InvalidState(e.to_string()))?
            .ok_or_else(|| ModuleRuntimeError::NotInstalled {
                module_id: module_id.to_string(),
            })?;

        if self.is_unmanaged_module(module_id) {
            return Ok(ModuleRuntimeInfo {
                module_id: module_id.clone(),
                version: ModuleVersion(installed.manifest.version.clone()),
                status: ModuleRuntimeStatus::Stopped,
                pid: None,
                port: None,
                started_at: None,
                stopped_at: Some(SystemTime::now()),
                restart_count: 0,
            });
        }

        if self.is_dev_override_active(module_id).await {
            return Ok(ModuleRuntimeInfo {
                module_id: module_id.clone(),
                version: ModuleVersion(installed.manifest.version.clone()),
                status: ModuleRuntimeStatus::Running,
                pid: None,
                port: None,
                started_at: None,
                stopped_at: None,
                restart_count: 1,
            });
        }
        let info = self.runtime.restart(module_id).await?;
        self.update_module_service_status(
            module_id,
            &installed.manifest,
            ServiceStatus::Active,
            Some(
                info.pid
                    .map(|pid| format!("läuft (PID {pid})"))
                    .unwrap_or_else(|| "läuft".to_string()),
            ),
        );
        Ok(info)
    }

    /// Get logs from a running module
    pub async fn logs(
        &self,
        module_id: &ModuleId,
        tail: Option<usize>,
    ) -> Result<Vec<String>, ModuleRuntimeError> {
        self.runtime.logs(module_id, tail).await
    }

    async fn remember_dev_services(&self, module_id: &ModuleId, service_ids: Vec<String>) {
        let mut guard = self.dev_overrides.write().await;
        guard.insert(module_id.clone(), DevOverrideState { service_ids });
    }

    async fn clear_dev_services_if_any(&self, module_id: &ModuleId) -> bool {
        let removed = {
            let mut guard = self.dev_overrides.write().await;
            guard.remove(module_id)
        };
        if let Some(state) = removed {
            for id in state.service_ids {
                self.service_registry.unregister(&id);
            }
            true
        } else {
            false
        }
    }

    async fn remember_declared_services(&self, module_id: &ModuleId, service_ids: Vec<String>) {
        let mut guard = self.declared_services.write().await;
        guard.insert(
            module_id.clone(),
            DeclaredServicesState {
                service_ids: service_ids.clone(),
            },
        );
    }

    async fn clear_declared_services_if_any(&self, module_id: &ModuleId) -> bool {
        let removed = {
            let mut guard = self.declared_services.write().await;
            guard.remove(module_id)
        };
        if let Some(state) = removed {
            for id in state.service_ids {
                self.service_registry.unregister(&id);
            }
            true
        } else {
            false
        }
    }

    #[allow(dead_code)]
    async fn has_service_bindings(&self, module_id: &ModuleId) -> bool {
        let dev = self.dev_overrides.read().await.contains_key(module_id);
        let declared = self.declared_services.read().await.contains_key(module_id);
        dev || declared
    }

    fn locate_dev_source(&self, module_id: &ModuleId) -> ModuleResult<Option<DevSourceInfo>> {
        let Some(config) = &self.dev_sources else {
            return Ok(None);
        };

        let module_root = config.module_root(module_id);
        if !module_root.exists() {
            return Ok(None);
        }
        if !module_root.is_dir() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Dev-Verzeichnis {} für Modul {} ist kein Ordner",
                    module_root.display(),
                    module_id
                )),
            ));
        }

        let override_definition = load_dev_override(&module_root)?;
        let config_path = override_definition.config_path.clone();
        let services = parse_dev_services(&override_definition.services, &config_path)?;
        let allow_missing_output = !services.is_empty();
        let package_source = detect_package_source(
            &module_root,
            override_definition.output_path,
            &config_path,
            allow_missing_output,
        )?;

        if package_source.is_none() && services.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Dev-Verzeichnis {} gefunden, aber kein Build-Ordner (dist/, build/, target/*) vorhanden. Lege eine `.fenrir-dev.toml` mit `output = \"pfad\"` an.",
                    module_root.display()
                )),
            ));
        }

        Ok(Some(DevSourceInfo {
            package_source,
            services,
        }))
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

async fn package_module_directory(module_dir: PathBuf) -> ModuleResult<Vec<u8>> {
    if !module_dir.exists() {
        return Err(ModuleServiceError::Storage(
            ModuleStorageError::InvalidState(format!(
                "Verzeichnis {} existiert nicht",
                module_dir.display()
            )),
        ));
    }
    if !module_dir.is_dir() {
        return Err(ModuleServiceError::Storage(
            ModuleStorageError::InvalidState(format!(
                "{} ist kein Verzeichnis",
                module_dir.display()
            )),
        ));
    }

    let bytes = task::spawn_blocking(move || -> Result<Vec<u8>, ModuleServiceError> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = Builder::new(encoder);
        append_directory_filtered(&mut builder, &module_dir, &module_dir)
            .map_err(|err| ModuleServiceError::Storage(ModuleStorageError::Io(err.to_string())))?;
        let encoder = builder
            .into_inner()
            .map_err(|err| ModuleServiceError::Storage(ModuleStorageError::Io(err.to_string())))?;
        encoder
            .finish()
            .map_err(|err| ModuleServiceError::Storage(ModuleStorageError::Io(err.to_string())))
    })
    .await
    .map_err(|err| ModuleServiceError::Storage(ModuleStorageError::Io(err.to_string())))??;

    Ok(bytes)
}

fn append_directory_filtered(
    builder: &mut Builder<GzEncoder<Vec<u8>>>,
    root: &Path,
    current: &Path,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .file_name()
            .map(|name| name == OsStr::new(".fenrir-meta"))
            .unwrap_or(false)
        {
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == ".fenrir-dev.toml")
            .unwrap_or(false)
        {
            continue;
        }

        let rel_path = path.strip_prefix(root).map_err(|err| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("failed to calculate relative path: {}", err),
            )
        })?;
        if path.is_dir() {
            builder.append_dir(rel_path, &path)?;
            append_directory_filtered(builder, root, &path)?;
        } else {
            builder.append_path_with_name(&path, rel_path)?;
        }
    }

    Ok(())
}

fn default_dev_output_candidates(root: &Path) -> Vec<PathBuf> {
    const DEV_FOLDERS: &[&[&str]] = &[
        &["dist"],
        &["build"],
        &["out"],
        &["output"],
        &["target", "release"],
        &["target", "debug"],
    ];

    DEV_FOLDERS
        .iter()
        .map(|segments| join_segments(root, segments))
        .chain(std::iter::once(root.to_path_buf()))
        .collect()
}

fn join_segments(root: &Path, segments: &[&str]) -> PathBuf {
    let mut path = root.to_path_buf();
    for segment in segments {
        path = path.join(segment);
    }
    path
}

fn load_dev_override(module_root: &Path) -> ModuleResult<DevOverrideDefinition> {
    let dev_path = module_root.join(".fenrir-dev.toml");
    let (config_path, contents) = if dev_path.exists() {
        let contents = fs::read_to_string(&dev_path).map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::Io(format!(
                "Dev-Konfiguration {} konnte nicht gelesen werden: {err}",
                dev_path.display()
            )))
        })?;
        (dev_path, contents)
    } else {
        let fallback = module_root.join(".fenrir").join("config.toml");
        if fallback.exists() {
            let contents = fs::read_to_string(&fallback).map_err(|err| {
                ModuleServiceError::Storage(ModuleStorageError::Io(format!(
                    "Dev-Konfiguration {} konnte nicht gelesen werden: {err}",
                    fallback.display()
                )))
            })?;
            (fallback, contents)
        } else {
            return Ok(DevOverrideDefinition {
                output_path: None,
                services: Vec::new(),
                config_path: module_root.join(".fenrir-dev.toml"),
            });
        }
    };

    let parsed: DevOverrideFile = toml::from_str(&contents).map_err(|err| {
        ModuleServiceError::Storage(ModuleStorageError::InvalidState(format!(
            "Dev-Konfiguration {} ist ungültig: {err}",
            config_path.display()
        )))
    })?;

    let output_path = if let Some(output) = parsed.output {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Dev-Konfiguration {} enthält einen leeren Output-Pfad",
                    config_path.display()
                )),
            ));
        }
        Some(module_root.join(trimmed))
    } else {
        None
    };

    Ok(DevOverrideDefinition {
        output_path,
        services: parsed.services,
        config_path,
    })
}

fn load_declared_services(module_root: &Path) -> ModuleResult<Vec<DevServiceBinding>> {
    let definition = load_dev_override(module_root)?;
    parse_dev_services(&definition.services, &definition.config_path)
}

fn detect_package_source(
    module_root: &Path,
    override_output: Option<PathBuf>,
    config_path: &Path,
    allow_none: bool,
) -> ModuleResult<Option<PathBuf>> {
    if let Some(custom) = override_output {
        if !custom.exists() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Dev-Konfiguration {} verweist auf nicht existierendes Verzeichnis {}",
                    config_path.display(),
                    custom.display()
                )),
            ));
        }
        if !custom.is_dir() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Dev-Konfiguration {} verweist auf {} (kein Verzeichnis)",
                    config_path.display(),
                    custom.display()
                )),
            ));
        }
        return Ok(Some(custom));
    }

    for candidate in default_dev_output_candidates(module_root) {
        if candidate.is_dir() {
            return Ok(Some(candidate));
        }
    }

    if allow_none {
        Ok(None)
    } else {
        Err(ModuleServiceError::Storage(
            ModuleStorageError::InvalidState(format!(
                "Dev-Verzeichnis {} gefunden, aber kein Build-Ordner (dist/, build/, target/*) vorhanden. Lege eine `.fenrir-dev.toml` mit `output = \"pfad\"` an.",
                module_root.display()
            )),
        ))
    }
}

fn parse_dev_services(
    definitions: &[DevOverrideServiceDefinition],
    config_path: &Path,
) -> ModuleResult<Vec<DevServiceBinding>> {
    let mut services = Vec::new();
    for definition in definitions {
        let id = definition.id.trim();
        if id.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Dev-Konfiguration {} enthält einen Service ohne id",
                    config_path.display()
                )),
            ));
        }
        let endpoint_raw = definition.endpoint.trim();
        if endpoint_raw.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(format!(
                    "Dev-Service {id} in {} benötigt einen Endpoint",
                    config_path.display()
                )),
            ));
        }
        let endpoint = endpoint_raw.parse::<SocketAddr>().map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(format!(
                "Dev-Service {id} in {} besitzt einen ungültigen Endpoint {}: {err}",
                config_path.display(),
                definition.endpoint
            )))
        })?;

        services.push(DevServiceBinding {
            id: id.to_string(),
            endpoint,
            name: definition.name.clone(),
            description: definition.description.clone(),
            kind: definition.kind.into(),
        });
    }
    Ok(services)
}

struct DevOverrideDefinition {
    output_path: Option<PathBuf>,
    services: Vec<DevOverrideServiceDefinition>,
    config_path: PathBuf,
}

#[derive(Debug, Deserialize, Default)]
struct DevOverrideFile {
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    services: Vec<DevOverrideServiceDefinition>,
    #[serde(flatten)]
    _extra: toml::value::Table,
}

#[derive(Debug, Deserialize)]
struct DevOverrideServiceDefinition {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    kind: DevOverrideServiceKind,
    endpoint: String,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum DevOverrideServiceKind {
    Infrastructure,
    Transport,
    BackgroundJob,
    Cli,
    Security,
    Storage,
    Other,
}

impl Default for DevOverrideServiceKind {
    fn default() -> Self {
        Self::Other
    }
}

impl From<DevOverrideServiceKind> for ServiceKind {
    fn from(value: DevOverrideServiceKind) -> Self {
        match value {
            DevOverrideServiceKind::Infrastructure => ServiceKind::Infrastructure,
            DevOverrideServiceKind::Transport => ServiceKind::Transport,
            DevOverrideServiceKind::BackgroundJob => ServiceKind::BackgroundJob,
            DevOverrideServiceKind::Cli => ServiceKind::Cli,
            DevOverrideServiceKind::Security => ServiceKind::Security,
            DevOverrideServiceKind::Storage => ServiceKind::Storage,
            DevOverrideServiceKind::Other => ServiceKind::Other,
        }
    }
}
