use std::ffi::OsStr;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use flate2::write::GzEncoder;
use flate2::Compression;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tar::Builder;
use tokio::task;

use crate::domain::module::{
    ChecksumAlgorithm, InstalledModule, ModuleBundle, ModuleId, ModuleInstallSource, ModuleResult,
    ModuleRuntimeError, ModuleServiceError, ModuleStorageError,
};
use crate::services::{ServiceDescriptorOwned, ServiceKind, ServiceStatus, ServiceTag};
use crate::utils::messages::services::module::{
    dev::{
        errors as module_dev_errors, logs as module_dev_logs, names as module_dev_names,
        notes as module_dev_notes,
    },
    service::errors as module_service_errors,
};

use super::types::{ModuleDevServices, ModuleSyncOutcome, ModuleSyncPackage, RegisteredDevService};
use super::ModuleService;

#[derive(Clone)]
pub(super) struct DevSourceConfig {
    base_path: PathBuf,
}

impl DevSourceConfig {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    pub fn module_root(&self, module_id: &ModuleId) -> PathBuf {
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
pub(super) struct DevOverrideState {
    service_ids: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct DeclaredServicesState {
    service_ids: Vec<String>,
}

impl ModuleService {
    /// Synchronize an installed module with the files located on this machine.
    pub async fn synchronize_from_local(
        &self,
        module_id: &ModuleId,
    ) -> ModuleResult<ModuleSyncOutcome> {
        let installed = self.storage.load(module_id).await?.ok_or_else(|| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(
                module_service_errors::module_missing(module_id),
            ))
        })?;

        if let Err(err) = self.stop_all_modules().await {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::stop_all_failed(err)),
            ));
        }

        if let Some(dev_info) = self.locate_dev_source(module_id)? {
            if !dev_info.services.is_empty() {
                tracing::info!(
                    module = %module_id,
                    services = dev_info.services.len(),
                    "{}",
                    module_dev_logs::ACTIVATING_DEV_OVERRIDE
                );
                return self
                    .activate_dev_services(module_id, &installed, dev_info.services)
                    .await;
            }

            if let Some(source) = dev_info.package_source {
                tracing::info!(
                    module = %module_id,
                    path = %source.display(),
                    "{}",
                    module_dev_logs::PACKAGING_FROM_DEV_SOURCES
                );
                return self
                    .install_from_directory(module_id, &installed, source)
                    .await;
            }
        }

        let module_dir = PathBuf::from(&installed.path);
        if !module_dir.exists() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::install_path_missing(
                    module_dir.display(),
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
                    "{}",
                    module_dev_logs::REGISTER_DECLARED_SERVICES_FAILED
                );
            }
        }

        Ok(ModuleSyncOutcome::Packaged(Box::new(ModuleSyncPackage {
            install_result: result,
            packaged_from: package_source,
        })))
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
                    "{}",
                    module_dev_logs::STOP_RUNTIME_FOR_DEV_FAILED
                );
            }
        }

        if bindings.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::no_dev_services(module_id)),
            ));
        }

        let mut registered = Vec::new();
        let mut ids = Vec::new();

        for binding in bindings {
            let service_id = format!("module:{}::{}", module_id, binding.id);
            let service_name = binding
                .name
                .clone()
                .unwrap_or_else(|| module_dev_names::fallback_service_name(module_id, &binding.id));
            let service_description = binding
                .description
                .clone()
                .unwrap_or_else(|| module_dev_names::default_dev_service().to_string());
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
                Some(module_dev_notes::dev_endpoint(binding.endpoint)),
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
            Some(module_dev_notes::DEV_OVERRIDE_ACTIVE.to_string()),
        );

        Ok(ModuleSyncOutcome::ExternalServices(ModuleDevServices {
            module_id: module_id.clone(),
            version: installed.manifest.module_version(),
            services: registered,
        }))
    }

    pub(super) async fn register_declared_services(
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
                .unwrap_or_else(|| module_dev_names::binding(&name, module_id));
            let descriptor =
                ServiceDescriptorOwned::new(service_id.clone(), name, description, binding.kind)
                    .with_tags(vec![ServiceTag::Auxiliary]);
            self.service_registry.register(
                descriptor,
                ServiceStatus::Active,
                Some(module_dev_notes::endpoint(binding.endpoint)),
            );
            ids.push(service_id);
        }

        self.remember_declared_services(module_id, ids).await;
        Ok(())
    }

    async fn remember_dev_services(&self, module_id: &ModuleId, service_ids: Vec<String>) {
        let mut guard = self.dev_overrides.write().await;
        guard.insert(module_id.clone(), DevOverrideState { service_ids });
    }

    pub(super) async fn clear_dev_services_if_any(&self, module_id: &ModuleId) -> bool {
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

    pub(super) async fn clear_declared_services_if_any(&self, module_id: &ModuleId) -> bool {
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
                ModuleStorageError::InvalidState(module_dev_errors::dev_root_not_directory(
                    module_root.display(),
                    module_id,
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
                ModuleStorageError::InvalidState(module_dev_errors::missing_build_dir(
                    module_root.display(),
                )),
            ));
        }

        Ok(Some(DevSourceInfo {
            package_source,
            services,
        }))
    }
}

async fn package_module_directory(module_dir: PathBuf) -> ModuleResult<Vec<u8>> {
    if !module_dir.exists() {
        return Err(ModuleServiceError::Storage(
            ModuleStorageError::InvalidState(module_dev_errors::directory_missing(
                module_dir.display(),
            )),
        ));
    }
    if !module_dir.is_dir() {
        return Err(ModuleServiceError::Storage(
            ModuleStorageError::InvalidState(module_dev_errors::not_a_directory(
                module_dir.display(),
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
            io::Error::other(format!("failed to calculate relative path: {}", err))
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
            ModuleServiceError::Storage(ModuleStorageError::Io(
                module_dev_errors::dev_config_read_failed(dev_path.display(), err),
            ))
        })?;
        (dev_path, contents)
    } else {
        let fallback = module_root.join(".fenrir").join("config.toml");
        if fallback.exists() {
            let contents = fs::read_to_string(&fallback).map_err(|err| {
                ModuleServiceError::Storage(ModuleStorageError::Io(
                    module_dev_errors::dev_config_read_failed(fallback.display(), err),
                ))
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
        ModuleServiceError::Storage(ModuleStorageError::InvalidState(
            module_dev_errors::dev_config_invalid(config_path.display(), err),
        ))
    })?;

    let output_path = if let Some(output) = parsed.output {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::dev_output_empty(
                    config_path.display(),
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
                ModuleStorageError::InvalidState(module_dev_errors::dev_output_missing(
                    config_path.display(),
                    custom.display(),
                )),
            ));
        }
        if !custom.is_dir() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::dev_output_not_dir(
                    config_path.display(),
                    custom.display(),
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
            ModuleStorageError::InvalidState(module_dev_errors::missing_build_dir(
                module_root.display(),
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
                ModuleStorageError::InvalidState(module_dev_errors::dev_service_missing_id(
                    config_path.display(),
                )),
            ));
        }
        let endpoint_raw = definition.endpoint.trim();
        if endpoint_raw.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::dev_service_missing_endpoint(
                    config_path.display(),
                    id,
                )),
            ));
        }
        let endpoint = endpoint_raw.parse::<SocketAddr>().map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(
                module_dev_errors::dev_service_invalid_endpoint(
                    config_path.display(),
                    id,
                    &definition.endpoint,
                    err,
                ),
            ))
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
