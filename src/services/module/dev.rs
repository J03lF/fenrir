use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use flate2::write::GzEncoder;
use flate2::Compression;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tar::Builder;
use tokio::{fs as tokio_fs, task};

use crate::domain::module::{
    ChecksumAlgorithm, InstalledModule, ModuleBundle, ModuleId, ModuleInstallSource, ModuleResult,
    ModuleRuntimeError, ModuleServiceError, ModuleStorageError,
};
use crate::security::service::{ServiceRole, ServiceScope};
use crate::services::{
    ServiceDescriptorOwned, ServiceIngressMetadata, ServiceIngressProtocol, ServiceKind,
    ServiceRateLimit, ServiceSecurityMetadata, ServiceStatus, ServiceTag, ServiceTenantGuard,
};
use crate::utils::messages::services::module::{
    dev::{
        errors as module_dev_errors, logs as module_dev_logs, names as module_dev_names,
        notes as module_dev_notes,
    },
    service::errors as module_service_errors,
};

use super::types::{
    ModuleDevRunState, ModuleDevServices, ModuleSyncOutcome, ModuleSyncPackage,
    RegisteredDevService,
};
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
    run: Option<DevRunConfig>,
}

#[derive(Debug, Clone)]
struct DevServiceBinding {
    id: String,
    endpoint: SocketAddr,
    name: Option<String>,
    description: Option<String>,
    kind: ServiceKind,
    security: ServiceSecurityMetadata,
    ingress: ServiceIngressMetadata,
}

#[derive(Debug, Clone)]
pub(super) struct DevRunConfig {
    pub(super) command: DevRunCommand,
    pub(super) workdir: Option<PathBuf>,
    pub(super) auto_restart: bool,
    pub(super) auto_start: bool,
    pub(super) env: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub(super) struct DevRunCommand {
    pub(super) args: Vec<String>,
    pub(super) display: String,
}

#[derive(Debug, Default)]
pub(super) struct DevOverrideState {
    pub(super) services: HashMap<String, SocketAddr>,
}

#[derive(Debug, Default)]
pub(super) struct DeclaredServicesState {
    pub(super) services: HashMap<String, SocketAddr>,
}

impl ModuleService {
    /// Synchronize an installed module with the files located on this machine.
    pub async fn synchronize_from_local(
        self: &Arc<Self>,
        module_id: &ModuleId,
    ) -> ModuleResult<ModuleSyncOutcome> {
        let installed = self.storage.load(module_id).await?.ok_or_else(|| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(
                module_service_errors::module_missing(module_id),
            ))
        })?;

        self.ensure_distribution_backup(module_id, &installed)
            .await?;

        self.stop_module_process(module_id).await;
        self.stop_dev_agent_if_any(module_id).await;

        if let Some(dev_info) = self.locate_dev_source(module_id)? {
            if !dev_info.services.is_empty() {
                tracing::info!(
                    module = %module_id,
                    services = dev_info.services.len(),
                    "{}",
                    module_dev_logs::ACTIVATING_DEV_OVERRIDE
                );
                let mut dev_services = self
                    .activate_dev_services(module_id, &installed, dev_info.services)
                    .await?;
                if let Some(run) = dev_info.run {
                    if run.auto_start {
                        match self.start_dev_agent(module_id, &dev_services, run).await {
                            Ok((agent_state, expires_at)) => {
                                dev_services.run = Some(agent_state);
                                self.register_dev_agent_rotation(module_id, expires_at)
                                    .await;
                            }
                            Err(err) => {
                                self.clear_dev_services_if_any(module_id).await;
                                return Err(err);
                            }
                        }
                    } else {
                        let command = run.command.display.clone();
                        let workdir = run
                            .workdir
                            .clone()
                            .or_else(|| self.dev_module_root(module_id))
                            .unwrap_or_else(|| PathBuf::from("."));
                        dev_services.run = Some(ModuleDevRunState {
                            command,
                            workdir,
                            auto_restart: run.auto_restart,
                            auto_start: false,
                            log_path: None,
                        });
                    }
                }
                return Ok(ModuleSyncOutcome::ExternalServices(dev_services));
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
        self.ensure_distribution_backup(module_id, installed)
            .await?;
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

        if let Err(err) = self.ensure_running(module_id).await {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::start_after_sync_failed(err)),
            ));
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
    ) -> ModuleResult<ModuleDevServices> {
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
        self.revoke_service_token_if_any(module_id, "dev-override")
            .await;

        if bindings.is_empty() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::no_dev_services(module_id)),
            ));
        }

        let mut registered = Vec::new();
        let mut remembered = Vec::new();

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
            let mut descriptor = ServiceDescriptorOwned::new(
                service_id.clone(),
                service_name.clone(),
                service_description,
                binding.kind,
            )
            .with_tags(vec![ServiceTag::Auxiliary])
            .with_security(binding.security.clone())
            .with_ingress(binding.ingress.clone());
            descriptor = self.apply_descriptor_overrides(descriptor);
            let adjusted_security = descriptor.security.clone();
            let adjusted_ingress = descriptor.ingress.clone();
            let endpoint = binding.endpoint;
            self.service_registry.register(
                descriptor,
                ServiceStatus::Active,
                Some(module_dev_notes::dev_endpoint(endpoint)),
            );

            registered.push(RegisteredDevService {
                service_id: service_id.clone(),
                endpoint,
                name: service_name,
                description: binding.description.clone(),
                kind: binding.kind,
                security: adjusted_security.clone(),
                ingress: adjusted_ingress.clone(),
            });
            remembered.push((service_id, endpoint));
        }

        self.remember_dev_services(module_id, remembered).await;
        self.update_module_service_status(
            module_id,
            &installed.manifest,
            ServiceStatus::Standby,
            Some(module_dev_notes::DEV_OVERRIDE_ACTIVE.to_string()),
        );

        Ok(ModuleDevServices {
            module_id: module_id.clone(),
            version: installed.manifest.module_version(),
            services: registered,
            run: None,
        })
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

        let mut remembered = Vec::new();
        for binding in declared {
            let service_id = binding.id.clone();
            let name = binding.name.clone().unwrap_or_else(|| binding.id.clone());
            let description = binding
                .description
                .clone()
                .unwrap_or_else(|| module_dev_names::binding(&name, module_id));
            let endpoint = binding.endpoint;
            let mut descriptor =
                ServiceDescriptorOwned::new(service_id.clone(), name, description, binding.kind)
                    .with_tags(vec![ServiceTag::Auxiliary])
                    .with_security(binding.security.clone())
                    .with_ingress(binding.ingress.clone());
            descriptor = self.apply_descriptor_overrides(descriptor);
            self.service_registry.register(
                descriptor,
                ServiceStatus::Active,
                Some(module_dev_notes::endpoint(endpoint)),
            );
            remembered.push((service_id, endpoint));
        }

        self.remember_declared_services(module_id, remembered).await;
        Ok(())
    }

    async fn remember_dev_services(
        &self,
        module_id: &ModuleId,
        service_ids: Vec<(String, SocketAddr)>,
    ) {
        let mut guard = self.dev_overrides.write().await;
        let mut services = HashMap::new();
        for (id, endpoint) in service_ids {
            services.insert(id, endpoint);
        }
        guard.insert(module_id.clone(), DevOverrideState { services });
    }

    pub(super) async fn clear_dev_services_if_any(&self, module_id: &ModuleId) -> bool {
        let removed = {
            let mut guard = self.dev_overrides.write().await;
            guard.remove(module_id)
        };
        if let Some(state) = removed {
            for id in state.services.keys() {
                self.service_registry.unregister(id);
            }
            self.stop_dev_agent_if_any(module_id).await;
            self.cancel_manual_token_rotation(module_id).await;
            self.cleanup_dev_artifacts(module_id).await;
            true
        } else {
            false
        }
    }

    async fn cleanup_dev_artifacts(&self, module_id: &ModuleId) {
        let Some(root) = self.dev_module_root(module_id) else {
            return;
        };
        let fenrir_dir = root.join(".fenrir");
        if tokio_fs::metadata(&fenrir_dir).await.is_err() {
            return;
        }

        let agent_dir = fenrir_dir.join("dev-agent");
        if tokio_fs::metadata(&agent_dir).await.is_ok() {
            if let Err(err) = tokio_fs::remove_dir_all(&agent_dir).await {
                tracing::debug!(
                    module = %module_id,
                    error = %err,
                    "failed to remove dev-agent artifacts"
                );
            }
        }

        let shared_env = fenrir_dir.join("dev.env");
        if tokio_fs::remove_file(&shared_env).await.is_err() {
            // ignore missing file
        }

        let mut dir = match tokio_fs::read_dir(&fenrir_dir).await {
            Ok(dir) => dir,
            Err(err) => {
                tracing::debug!(
                    module = %module_id,
                    error = %err,
                    "failed to iterate env export directory"
                );
                return;
            }
        };

        while let Ok(Some(entry)) = dir.next_entry().await {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if name.starts_with("dev-env-") && name.ends_with(".sh") {
                if let Err(err) = tokio_fs::remove_file(entry.path()).await {
                    tracing::debug!(
                        module = %module_id,
                        file = %name,
                        error = %err,
                        "failed to remove env export"
                    );
                }
            }
        }
    }

    async fn remember_declared_services(
        &self,
        module_id: &ModuleId,
        service_ids: Vec<(String, SocketAddr)>,
    ) {
        let mut guard = self.declared_services.write().await;
        let mut services = HashMap::new();
        for (id, endpoint) in service_ids {
            services.insert(id, endpoint);
        }
        guard.insert(module_id.clone(), DeclaredServicesState { services });
    }

    pub(super) async fn clear_declared_services_if_any(&self, module_id: &ModuleId) -> bool {
        let removed = {
            let mut guard = self.declared_services.write().await;
            guard.remove(module_id)
        };
        if let Some(state) = removed {
            for id in state.services.keys() {
                self.service_registry.unregister(id);
            }
            true
        } else {
            false
        }
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
            run: override_definition.run,
        }))
    }
}

pub(super) async fn package_module_directory(module_dir: PathBuf) -> ModuleResult<Vec<u8>> {
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
                run: None,
            });
        }
    };

    let parsed: DevOverrideFile = toml::from_str(&contents).map_err(|err| {
        ModuleServiceError::Storage(ModuleStorageError::InvalidState(
            module_dev_errors::dev_config_invalid(config_path.display(), err),
        ))
    })?;

    let output_path = if let Some(output) = parsed.output.clone() {
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

    let run = parse_dev_run(
        parsed.dev.as_ref().and_then(|section| section.run.clone()),
        module_root,
        &config_path,
    )?;

    Ok(DevOverrideDefinition {
        output_path,
        services: parsed.services,
        config_path,
        run,
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

        let allowed_roles = if definition.allowed_roles.is_empty() {
            ServiceSecurityMetadata::default_allowed_roles()
        } else {
            let mut roles = Vec::new();
            for role in &definition.allowed_roles {
                match ServiceRole::from_str(role) {
                    Ok(parsed) => roles.push(parsed),
                    Err(_) => {
                        return Err(ModuleServiceError::Storage(
                            ModuleStorageError::InvalidState(
                                module_dev_errors::dev_service_invalid_role(
                                    config_path.display(),
                                    id,
                                    role,
                                ),
                            ),
                        ));
                    }
                }
            }
            roles
        };

        let mut scopes = Vec::new();
        for scope in &definition.required_scopes {
            match ServiceScope::new(scope) {
                Ok(parsed) => scopes.push(parsed),
                Err(err) => {
                    return Err(ModuleServiceError::Storage(
                        ModuleStorageError::InvalidState(
                            module_dev_errors::dev_service_invalid_scope(
                                config_path.display(),
                                id,
                                scope,
                                err,
                            ),
                        ),
                    ));
                }
            }
        }

        let security = ServiceSecurityMetadata {
            internal_only: definition.internal_only.unwrap_or(true),
            allowed_roles,
            required_scopes: scopes,
            tenant: ServiceTenantGuard::any(),
        };

        let mut ingress = match definition
            .access
            .unwrap_or(DevOverrideIngressAccess::Internal)
        {
            DevOverrideIngressAccess::Internal => ServiceIngressMetadata::internal(),
            DevOverrideIngressAccess::Public => ServiceIngressMetadata::public(),
        };
        if let Some(prefix) = definition.route_prefix.as_deref() {
            ingress = ingress.with_route_prefix(prefix);
        }
        if let Some(health) = definition.health_endpoint.as_deref() {
            ingress = ingress.with_health_endpoint(health);
        }
        let rate_limit = if definition.disable_rate_limit.unwrap_or(false) {
            ServiceRateLimit::Unlimited
        } else if let Some(limit) = definition.rate_limit_per_second {
            if limit == 0 {
                return Err(ModuleServiceError::Storage(
                    ModuleStorageError::InvalidState(
                        module_dev_errors::dev_service_invalid_rate_limit(
                            config_path.display(),
                            id,
                            limit,
                        ),
                    ),
                ));
            }
            ServiceRateLimit::CustomPerSecond(limit)
        } else {
            ServiceRateLimit::Default
        };
        ingress = ingress.with_rate_limit(rate_limit);
        let protocols = if definition.protocols.is_empty() {
            vec![DevOverrideServiceProtocol::Http]
        } else {
            definition.protocols.clone()
        };
        let mapped = protocols
            .into_iter()
            .map(ServiceIngressProtocol::from)
            .collect::<Vec<_>>();
        ingress = ingress.with_protocols(mapped);

        services.push(DevServiceBinding {
            id: id.to_string(),
            endpoint,
            name: definition.name.clone(),
            description: definition.description.clone(),
            kind: definition.kind.into(),
            security,
            ingress,
        });
    }
    Ok(services)
}

fn parse_dev_run(
    definition: Option<DevRunDefinition>,
    module_root: &Path,
    config_path: &Path,
) -> ModuleResult<Option<DevRunConfig>> {
    let Some(definition) = definition else {
        return Ok(None);
    };
    let command_value = definition.command.ok_or_else(|| {
        ModuleServiceError::Storage(ModuleStorageError::InvalidState(
            module_dev_errors::dev_run_missing_command(config_path.display()),
        ))
    })?;
    let command = normalize_dev_command(command_value, config_path)?;
    let workdir = if let Some(raw) = definition.workdir {
        let candidate = PathBuf::from(raw);
        let resolved = if candidate.is_absolute() {
            candidate
        } else {
            module_root.join(candidate)
        };
        if !resolved.exists() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::dev_run_workdir_missing(
                    config_path.display(),
                    resolved.display(),
                )),
            ));
        }
        Some(resolved)
    } else {
        None
    };

    let auto_restart = definition.auto_restart.unwrap_or(true);
    let auto_start = definition.auto_start.unwrap_or(true);
    let env = definition.env;

    Ok(Some(DevRunConfig {
        command,
        workdir,
        auto_restart,
        auto_start,
        env,
    }))
}

fn normalize_dev_command(
    value: DevRunCommandValue,
    config_path: &Path,
) -> ModuleResult<DevRunCommand> {
    let (args, display) = match value {
        DevRunCommandValue::String(cmd) => {
            let display = cmd.clone();
            (shell_wrapped_command(cmd), display)
        }
        DevRunCommandValue::List(list) => {
            if list.is_empty() {
                return Err(ModuleServiceError::Storage(
                    ModuleStorageError::InvalidState(module_dev_errors::dev_run_invalid_command(
                        config_path.display(),
                    )),
                ));
            }
            let display = list.join(" ");
            (list, display)
        }
    };
    if args.is_empty() {
        return Err(ModuleServiceError::Storage(
            ModuleStorageError::InvalidState(module_dev_errors::dev_run_invalid_command(
                config_path.display(),
            )),
        ));
    }
    Ok(DevRunCommand { args, display })
}

fn shell_wrapped_command(script: String) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".to_string(), "/C".to_string(), script]
    } else {
        vec!["/bin/sh".to_string(), "-c".to_string(), script]
    }
}

struct DevOverrideDefinition {
    output_path: Option<PathBuf>,
    services: Vec<DevOverrideServiceDefinition>,
    config_path: PathBuf,
    run: Option<DevRunConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct DevOverrideFile {
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    services: Vec<DevOverrideServiceDefinition>,
    #[serde(default)]
    dev: Option<DevOverrideDevSection>,
    #[serde(flatten)]
    _extra: toml::value::Table,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct DevOverrideDevSection {
    #[serde(default)]
    run: Option<DevRunDefinition>,
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
    #[serde(default)]
    internal_only: Option<bool>,
    #[serde(default)]
    allowed_roles: Vec<String>,
    #[serde(default)]
    required_scopes: Vec<String>,
    #[serde(default)]
    route_prefix: Option<String>,
    #[serde(default)]
    health_endpoint: Option<String>,
    #[serde(default)]
    access: Option<DevOverrideIngressAccess>,
    #[serde(default)]
    protocols: Vec<DevOverrideServiceProtocol>,
    #[serde(default)]
    rate_limit_per_second: Option<u32>,
    #[serde(default)]
    disable_rate_limit: Option<bool>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct DevRunDefinition {
    #[serde(default)]
    command: Option<DevRunCommandValue>,
    #[serde(default)]
    workdir: Option<String>,
    #[serde(default)]
    auto_restart: Option<bool>,
    #[serde(default)]
    auto_start: Option<bool>,
    #[serde(default)]
    env: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
enum DevRunCommandValue {
    String(String),
    List(Vec<String>),
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

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum DevOverrideIngressAccess {
    Internal,
    Public,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum DevOverrideServiceProtocol {
    Http,
    Grpc,
}

impl From<DevOverrideServiceProtocol> for ServiceIngressProtocol {
    fn from(value: DevOverrideServiceProtocol) -> Self {
        match value {
            DevOverrideServiceProtocol::Http => ServiceIngressProtocol::Http,
            DevOverrideServiceProtocol::Grpc => ServiceIngressProtocol::Grpc,
        }
    }
}
