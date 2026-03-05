use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock as SyncRwLock, Weak};
use std::time::{Duration, Instant, SystemTime};

use crate::audit::AuditOutcome;
use crate::dev_agent::config::{DevAgentConfig, DevAgentService};
use crate::domain::module::{
    ChecksumAlgorithm, DistributionTarget, InstalledModule, ModuleBundle, ModuleId,
    ModuleInstallResult, ModuleInstallSource, ModuleInstallStatus, ModuleManifest,
    ModuleRegistryPort, ModuleResult, ModuleRuntimeError, ModuleRuntimePort, ModuleRuntimeStatus,
    ModuleSearchQuery, ModuleServiceError, ModuleStorageError, ModuleStoragePort, ModuleSummary,
    ModuleVerifierPort, ModuleVersion, ProgressCallback,
};
use crate::security::manager::SecurityManager;
use crate::security::service::{ServiceRole, ServiceScope};
use crate::security::service_tokens::{DelegatedActor, DelegatedToken, DelegatedTokenRequest};
use crate::services::AppServices;
use crate::services::{
    diagnostics::ServiceDiagnostics, DbConnectorEndpoint, ServiceDescriptorOwned,
    ServiceIngressMetadata, ServiceKind, ServiceRegistry, ServiceSecurityMetadata, ServiceStatus,
    ServiceTag, ServiceTenantGuard,
};
use crate::utils::messages::services::module::{
    dev::errors as module_dev_errors,
    scaffold::errors as module_scaffold_errors,
    service::{
        errors as module_service_errors, logs as module_service_logs, notes as module_service_notes,
    },
};
use crate::utils::system_time_to_rfc3339;
use reqwest::Client;
use serde_json::{self, Value as JsonValue};
use sha2::{Digest, Sha256};
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio::{
    fs as tokio_fs,
    sync::{Mutex, RwLock},
};

use super::clients::{ModuleClientSettings, ModuleHealthHttpClient};
use super::config::ModuleServiceOverrides;
use super::dev::{
    package_module_directory, DeclaredServicesState, DevOverrideState, DevRunConfig,
    DevSourceConfig,
};
use super::dev_agent::DevAgentHandle;
use super::dev_env::write_plain_env_file;
use super::gateway::{GatewaySettings, RuntimeGatewayRegistry};
use super::ports::ModulePortAllocator;
use super::scaffold::generate_module_scaffold;
use super::token_audit::{record_module_token_exchange, ModuleTokenAuditContext};
use super::types::{
    DistributionAction, DistributionPlanEntry, ModuleIngressError, ModuleIngressTarget,
    ModuleReleaseOutcome, ModuleScaffoldOptions, ModuleScaffoldSummary, ModuleStartupReport,
    ModuleUpdateInfo, RegisteredDevService,
};

const DEFAULT_SERVICE_TENANT: &str = "default";
const FAILURE_WINDOW_SECS: u64 = 120;
const FAILURE_THRESHOLD: u32 = 3;
const QUARANTINE_DURATION_SECS: u64 = 300;
pub(super) const MODULE_SERVICE_MANIFEST_PATH: &str = "/.fenrir/services";
pub(super) const RESERVED_ENV_KEYS: [&str; 15] = [
    "FENRIR_MODULE_ID",
    "FENRIR_SERVICE_ID",
    "FENRIR_SERVICE_URI",
    "FENRIR_SERVICE_PORT",
    "FENRIR_SERVICE_ADDR",
    "FENRIR_SERVICE_TOKEN",
    "FENRIR_SERVICE_TOKEN_ISSUED_AT",
    "FENRIR_SERVICE_TOKEN_EXPIRES_AT",
    "FENRIR_SERVICE_TOKEN_TTL_SECS",
    "FENRIR_DB_CONNECTOR_PROTOCOL",
    "FENRIR_DB_CONNECTOR_ENDPOINT",
    "FENRIR_DB_CONNECTOR_URI",
    "FENRIR_CONTROL_PLANE_URL",
    "FENRIR_SERVICE_SNAPSHOT_PATH",
    "FENRIR_GATEWAY_ENDPOINT",
];
const DISTRIBUTION_BACKUP_DIR: &str = ".fenrir-backups";
const BACKUP_MANIFEST_FILE: &str = "manifest.json";
const BACKUP_ARCHIVE_FILE: &str = "archive.bin";
const BACKUP_SIGNATURE_FILE: &str = "signature.bin";
const BACKUP_CHECKSUM_FILE: &str = "checksum.bin";
const DEV_AGENT_TOKEN_REFRESH_LEAD_SECS: i64 = 60;
const DEV_AGENT_TOKEN_RETRY_SECS: i64 = 30;

pub struct RuntimeEnvironmentExport {
    pub entries: Vec<(String, String)>,
    pub token: DelegatedToken,
}

#[derive(Default)]
pub(super) struct ModuleHealth {
    failure_count: u32,
    last_failure: Option<SystemTime>,
    quarantined_until: Option<SystemTime>,
}

#[derive(Clone)]
pub struct ModuleTokenLease {
    token: String,
    issued_at: OffsetDateTime,
    expires_at: OffsetDateTime,
    scopes: Vec<ServiceScope>,
}

impl ModuleTokenLease {
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn issued_at(&self) -> OffsetDateTime {
        self.issued_at
    }

    pub fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    pub fn scopes(&self) -> &[ServiceScope] {
        &self.scopes
    }

    pub fn seconds_until_expiry(&self) -> i64 {
        (self.expires_at - OffsetDateTime::now_utc()).whole_seconds()
    }

    pub fn is_expired(&self) -> bool {
        self.seconds_until_expiry() <= 0
    }
}

impl From<&DelegatedToken> for ModuleTokenLease {
    fn from(token: &DelegatedToken) -> Self {
        Self {
            token: token.token.clone(),
            issued_at: token.claims.issued_at,
            expires_at: token.claims.expires_at,
            scopes: token.claims.scopes.clone(),
        }
    }
}

pub struct ModuleServiceInit {
    pub registry: Arc<dyn ModuleRegistryPort>,
    pub storage: Arc<dyn ModuleStoragePort>,
    pub verifier: Arc<dyn ModuleVerifierPort>,
    pub runtime: Arc<dyn ModuleRuntimePort>,
    pub service_registry: Arc<ServiceRegistry>,
    pub port_allocator: Arc<ModulePortAllocator>,
    pub security: Arc<SecurityManager>,
    pub dev_sources: Option<PathBuf>,
    pub overrides: ModuleServiceOverrides,
    pub client_settings: ModuleClientSettings,
    pub health_client: ModuleHealthHttpClient,
    pub default_service_scopes: Vec<ServiceScope>,
    pub env_passthrough_prefixes: Vec<String>,
    pub control_plane_url: Option<String>,
    pub service_snapshot_path: Option<PathBuf>,
    pub diagnostics: Arc<ServiceDiagnostics>,
    pub services: Weak<AppServices>,
}

pub struct ModuleService {
    pub(super) module_registry: Arc<dyn ModuleRegistryPort>,
    pub(super) storage: Arc<dyn ModuleStoragePort>,
    pub(super) verifier: Arc<dyn ModuleVerifierPort>,
    pub(super) runtime: Arc<dyn ModuleRuntimePort>,
    pub(super) service_registry: Arc<ServiceRegistry>,
    pub(super) port_allocator: Arc<ModulePortAllocator>,
    pub(super) security: Arc<SecurityManager>,
    pub(super) dev_sources: Option<DevSourceConfig>,
    pub(super) overrides: Arc<ModuleServiceOverrides>,
    pub(super) client_settings: ModuleClientSettings,
    pub(super) health_client: ModuleHealthHttpClient,
    pub(super) diagnostics: Arc<ServiceDiagnostics>,
    pub(super) runtime_gateways: Arc<RuntimeGatewayRegistry>,
    pub(super) dev_overrides: Arc<RwLock<HashMap<ModuleId, DevOverrideState>>>,
    pub(super) dev_agents: Arc<Mutex<HashMap<ModuleId, Arc<Mutex<DevAgentHandle>>>>>,
    pub(super) dev_agent_rotations: Arc<Mutex<HashMap<ModuleId, JoinHandle<()>>>>,
    pub(super) manual_token_rotations: Arc<Mutex<HashMap<ModuleId, JoinHandle<()>>>>,
    pub(super) manifest_refresh_tasks: Arc<Mutex<HashMap<ModuleId, JoinHandle<()>>>>,
    pub(super) declared_services: Arc<RwLock<HashMap<ModuleId, DeclaredServicesState>>>,
    pub(super) runtime_services: Arc<RwLock<HashMap<ModuleId, Vec<String>>>>,
    pub(super) service_tokens: Arc<RwLock<HashMap<ModuleId, ModuleTokenLease>>>,
    pub(super) db_connector_endpoint: SyncRwLock<Option<DbConnectorEndpoint>>,
    pub(super) default_service_scopes: Vec<ServiceScope>,
    pub(super) env_passthrough_prefixes: Vec<String>,
    pub(super) control_plane_url: SyncRwLock<Option<String>>,
    pub(super) health: SyncRwLock<HashMap<ModuleId, ModuleHealth>>,
    pub(super) manifest_client: Client,
    pub(super) service_snapshot_path: Option<PathBuf>,
    pub(super) service_endpoints: Arc<RwLock<HashMap<String, String>>>,
    pub(super) startup_reports: Arc<RwLock<HashMap<ModuleId, ModuleStartupReport>>>,
    pub(super) app_services: Weak<AppServices>,
    pub(super) self_ref: Weak<ModuleService>,
}

impl ModuleService {
    pub fn dev_module_root(&self, module_id: &ModuleId) -> Option<PathBuf> {
        self.dev_sources
            .as_ref()
            .map(|config| config.module_root(module_id))
    }

    pub async fn list_installed_modules(
        &self,
    ) -> Result<Vec<crate::domain::module::InstalledModule>, ModuleServiceError> {
        self.storage
            .list()
            .await
            .map_err(ModuleServiceError::Storage)
    }

    pub async fn install_module(
        &self,
        id: &ModuleId,
    ) -> ModuleResult<crate::domain::module::ModuleInstallResult> {
        let manifest = self.module_registry.fetch_manifest(id, None).await?;
        let bundle = self.module_registry.download(&manifest).await?;
        self.verifier.verify(&bundle).await?;
        self.storage
            .stage_and_activate(
                bundle,
                crate::domain::module::ModuleInstallSource::Distribution,
            )
            .await
            .map_err(ModuleServiceError::Storage)
    }

    pub async fn uninstall_module(&self, id: &ModuleId) -> ModuleResult<()> {
        let _ = self.stop(id).await;
        self.storage
            .remove(id)
            .await
            .map_err(ModuleServiceError::Storage)
    }

    pub async fn scaffold_module(
        &self,
        module_id: &ModuleId,
        options: ModuleScaffoldOptions,
    ) -> ModuleResult<ModuleScaffoldSummary> {
        let dev_sources = self.dev_sources.as_ref().ok_or_else(|| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(
                module_scaffold_errors::DEV_SOURCES_DISABLED.to_string(),
            ))
        })?;
        let root = dev_sources.module_root(module_id);
        if tokio_fs::metadata(&root).await.is_ok() {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_scaffold_errors::module_exists(
                    root.display(),
                )),
            ));
        }
        generate_module_scaffold(module_id, root, options).await
    }

    pub async fn issue_module_service_token(
        &self,
        module_id: &ModuleId,
    ) -> Result<Option<DelegatedToken>, ModuleRuntimeError> {
        if self.is_unmanaged_module(module_id) {
            return Ok(None);
        }
        if self.is_dev_override_active(module_id).await {
            return Ok(None);
        }
        self.issue_default_service_token(module_id).map(Some)
    }

    pub fn issue_default_service_token(
        &self,
        module_id: &ModuleId,
    ) -> Result<DelegatedToken, ModuleRuntimeError> {
        self.issue_service_token_with_scopes(module_id, self.default_service_scopes.clone())
    }

    pub fn spawn_health_monitor(self: &Arc<Self>) {
        let interval = self.client_settings.health_interval;
        if interval.is_zero() {
            return;
        }
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service.run_health_monitor(interval).await;
        });
    }

    async fn run_health_monitor(self: Arc<Self>, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        loop {
            ticker.tick().await;
            let started_at = Instant::now();
            match self.run_health_probe_cycle().await {
                Ok(_) => {
                    let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
                    self.diagnostics
                        .record_probe("module-runtime", latency_ms, true);
                }
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "{}",
                        module_service_logs::HEALTH_PROBE_FAILED
                    );
                    let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
                    self.diagnostics
                        .record_probe("module-runtime", latency_ms, false);
                }
            }
        }
    }

    async fn run_health_probe_cycle(&self) -> Result<(), ModuleRuntimeError> {
        let running = self.runtime.list_running().await?;
        for info in running {
            if let Some(port) = info.port {
                self.probe_runtime_service(&info.module_id, port).await;
            }
        }
        Ok(())
    }

    pub async fn verify_runtime_health(&self) -> Result<(), ModuleRuntimeError> {
        self.run_health_probe_cycle().await
    }

    async fn probe_runtime_service(&self, module_id: &ModuleId, port: u16) {
        let service_id = Self::module_service_id(module_id);
        let Some(snapshot) = self.service_registry.get(&service_id) else {
            return;
        };
        let health_path = snapshot
            .descriptor
            .ingress
            .as_ref()
            .and_then(|meta| meta.health_endpoint.clone())
            .unwrap_or_else(|| "/health".to_string());
        let url = format!("http://127.0.0.1:{port}{health_path}");
        let started_at = Instant::now();
        match self.health_client.check(&url).await {
            Ok(()) => {
                let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
                self.diagnostics.record_probe(&service_id, latency_ms, true);
                if snapshot.status != ServiceStatus::Active {
                    self.service_registry.set_status(
                        &service_id,
                        ServiceStatus::Active,
                        Some(module_service_notes::HEALTHY.to_string()),
                    );
                }
            }
            Err(err) => {
                let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
                self.diagnostics
                    .record_probe(&service_id, latency_ms, false);
                self.service_registry.set_status(
                    &service_id,
                    ServiceStatus::Degraded,
                    Some(format!("health probe failed: {err}")),
                );
            }
        }
    }

    pub async fn export_runtime_environment(
        &self,
        module_id: &ModuleId,
        endpoint: Option<SocketAddr>,
    ) -> ModuleResult<RuntimeEnvironmentExport> {
        let gateway_endpoint = self
            .ensure_gateway_endpoint(module_id)
            .await
            .map_err(|err| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
            })?;
        let issued = self
            .issue_service_token_with_scopes(module_id, self.default_service_scopes.clone())
            .map_err(|err| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
            })?;
        self.record_service_token(module_id, &issued).await;
        let port = endpoint.map(|addr| addr.port());
        let mut env = self
            .inject_runtime_env(Vec::new(), module_id, port, Some(&issued))
            .map_err(|err| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
            })?;
        if let Some(endpoint) = endpoint {
            let addr = endpoint.to_string();
            env.retain(|(key, _)| key != "FENRIR_SERVICE_ADDR");
            env.push(("FENRIR_SERVICE_ADDR".to_string(), addr));
        }
        env.retain(|(key, _)| key != "FENRIR_GATEWAY_ENDPOINT");
        env.push(("FENRIR_GATEWAY_ENDPOINT".to_string(), gateway_endpoint));
        Ok(RuntimeEnvironmentExport {
            entries: env,
            token: issued,
        })
    }

    pub(super) async fn register_dev_agent_rotation(
        self: &Arc<Self>,
        module_id: &ModuleId,
        expires_at: OffsetDateTime,
    ) {
        if let Some((run, services)) = self.dev_agent_context(module_id).await {
            self.schedule_dev_agent_rotation(module_id.clone(), run, services, expires_at)
                .await;
        }
    }

    async fn dev_agent_context(
        &self,
        module_id: &ModuleId,
    ) -> Option<(Arc<DevRunConfig>, Arc<Vec<RegisteredDevService>>)> {
        let handle_arc = {
            let guard = self.dev_agents.lock().await;
            guard.get(module_id).cloned()
        }?;
        let handle = handle_arc.lock().await;
        Some((Arc::clone(&handle.run), Arc::clone(&handle.services)))
    }

    async fn schedule_dev_agent_rotation(
        self: &Arc<Self>,
        module_id: ModuleId,
        run: Arc<DevRunConfig>,
        services: Arc<Vec<RegisteredDevService>>,
        expires_at: OffsetDateTime,
    ) {
        let service = Arc::clone(self);
        let module_id_for_task = module_id.clone();
        let handle = tokio::spawn(async move {
            let mut next_expiry = expires_at;
            loop {
                if !service.is_dev_agent_running(&module_id_for_task).await {
                    break;
                }
                let now = OffsetDateTime::now_utc();
                let refresh_at =
                    next_expiry - TimeDuration::seconds(DEV_AGENT_TOKEN_REFRESH_LEAD_SECS);
                let wait_duration = (refresh_at - now).max(TimeDuration::seconds(5));
                let sleep_duration = wait_duration
                    .try_into()
                    .unwrap_or_else(|_| std::time::Duration::from_secs(5));
                sleep(sleep_duration).await;
                if !service.is_dev_agent_running(&module_id_for_task).await {
                    break;
                }
                match service
                    .refresh_dev_agent_token(
                        &module_id_for_task,
                        Arc::clone(&run),
                        Arc::clone(&services),
                    )
                    .await
                {
                    Ok(expiry) => {
                        next_expiry = expiry;
                    }
                    Err(err) => {
                        tracing::warn!(
                            module = %module_id_for_task,
                            error = %err,
                            "failed to refresh dev agent token"
                        );
                        next_expiry = OffsetDateTime::now_utc()
                            + TimeDuration::seconds(DEV_AGENT_TOKEN_RETRY_SECS);
                    }
                }
            }
        });
        self.register_rotation_handle(&module_id, handle).await;
    }

    async fn register_rotation_handle(&self, module_id: &ModuleId, handle: JoinHandle<()>) {
        let mut guard = self.dev_agent_rotations.lock().await;
        if let Some(existing) = guard.insert(module_id.clone(), handle) {
            existing.abort();
        }
    }

    pub(super) async fn cancel_dev_agent_rotation(&self, module_id: &ModuleId) {
        let handle = {
            let mut guard = self.dev_agent_rotations.lock().await;
            guard.remove(module_id)
        };
        if let Some(handle) = handle {
            handle.abort();
        }
    }

    pub async fn enable_manual_token_rotation(
        self: &Arc<Self>,
        module_id: &ModuleId,
        env_path: PathBuf,
        endpoint: Option<SocketAddr>,
        expires_at: OffsetDateTime,
    ) -> ModuleResult<()> {
        self.cancel_manual_token_rotation(module_id).await;
        let service = Arc::clone(self);
        let module_id_for_task = module_id.clone();
        let handle = tokio::spawn(async move {
            service
                .run_manual_token_rotation(module_id_for_task, env_path, endpoint, expires_at)
                .await;
        });
        self.register_manual_rotation_handle(module_id, handle)
            .await;
        Ok(())
    }

    async fn register_manual_rotation_handle(&self, module_id: &ModuleId, handle: JoinHandle<()>) {
        let mut guard = self.manual_token_rotations.lock().await;
        if let Some(existing) = guard.insert(module_id.clone(), handle) {
            existing.abort();
        }
    }

    pub(super) async fn cancel_manual_token_rotation(&self, module_id: &ModuleId) {
        let handle = {
            let mut guard = self.manual_token_rotations.lock().await;
            guard.remove(module_id)
        };
        if let Some(handle) = handle {
            handle.abort();
        }
    }

    async fn run_manual_token_rotation(
        self: Arc<Self>,
        module_id: ModuleId,
        env_path: PathBuf,
        endpoint: Option<SocketAddr>,
        expires_at: OffsetDateTime,
    ) {
        let mut next_expiry = expires_at;
        loop {
            if !self.is_dev_override_active(&module_id).await {
                break;
            }
            let now = OffsetDateTime::now_utc();
            let refresh_at = next_expiry - TimeDuration::seconds(DEV_AGENT_TOKEN_REFRESH_LEAD_SECS);
            let wait_duration = (refresh_at - now).max(TimeDuration::seconds(5));
            let sleep_duration = wait_duration
                .try_into()
                .unwrap_or_else(|_| std::time::Duration::from_secs(5));
            sleep(sleep_duration).await;
            if !self.is_dev_override_active(&module_id).await {
                break;
            }
            match self
                .refresh_manual_env(&module_id, &env_path, endpoint)
                .await
            {
                Ok(expiry) => {
                    next_expiry = expiry;
                }
                Err(err) => {
                    tracing::warn!(
                        module = %module_id,
                        error = %err,
                        "failed to refresh manual dev env token"
                    );
                    next_expiry = OffsetDateTime::now_utc()
                        + TimeDuration::seconds(DEV_AGENT_TOKEN_RETRY_SECS);
                }
            }
        }
    }

    async fn refresh_manual_env(
        &self,
        module_id: &ModuleId,
        env_path: &PathBuf,
        endpoint: Option<SocketAddr>,
    ) -> ModuleResult<OffsetDateTime> {
        let export = self.export_runtime_environment(module_id, endpoint).await?;
        let mut env_values = export.entries.clone();
        env_values.push((
            "FENRIR_DEV_ENV_FILE".to_string(),
            env_path.display().to_string(),
        ));
        write_plain_env_file(env_path, &env_values)
            .map_err(|err| ModuleServiceError::Storage(ModuleStorageError::Io(err.to_string())))?;
        Ok(export.token.claims.expires_at)
    }

    async fn is_dev_agent_running(&self, module_id: &ModuleId) -> bool {
        self.dev_agents.lock().await.contains_key(module_id)
    }

    async fn refresh_dev_agent_token(
        self: &Arc<Self>,
        module_id: &ModuleId,
        run: Arc<DevRunConfig>,
        services: Arc<Vec<RegisteredDevService>>,
    ) -> ModuleResult<OffsetDateTime> {
        let primary_endpoint = services.first().map(|svc| svc.endpoint);
        let env_export = self
            .export_runtime_environment(module_id, primary_endpoint)
            .await?;
        let mut env_map: HashMap<String, String> = env_export.entries.into_iter().collect();
        for (key, value) in &run.env {
            if RESERVED_ENV_KEYS
                .iter()
                .any(|reserved| reserved == &key.as_str())
            {
                continue;
            }
            env_map.insert(key.clone(), value.clone());
        }

        let services_config = services
            .iter()
            .map(|svc| DevAgentService {
                id: svc.service_id.clone(),
                endpoint: svc.endpoint.to_string(),
            })
            .collect::<Vec<_>>();

        let handle_arc = {
            let guard = self.dev_agents.lock().await;
            guard.get(module_id).cloned()
        };
        let Some(handle_arc) = handle_arc else {
            return Err(ModuleServiceError::Storage(
                ModuleStorageError::InvalidState(module_dev_errors::dev_agent_missing(module_id)),
            ));
        };

        let mut handle = handle_arc.lock().await;
        let resolved_workdir = handle.workdir.clone();
        let config = DevAgentConfig {
            module_id: module_id.to_string(),
            command: run.command.args.clone(),
            command_display: run.command.display.clone(),
            workdir: resolved_workdir,
            auto_restart: run.auto_restart,
            env: env_map,
            log_path: handle.log_path.clone(),
            services: services_config,
        };

        Self::write_agent_config(&handle.config_path, &config).await?;

        if let Err(err) = handle.child.start_kill() {
            tracing::warn!(
                module = %module_id,
                error = %err,
                "failed to stop dev agent during token refresh"
            );
        }
        if let Err(err) = handle.child.wait().await {
            tracing::warn!(
                module = %module_id,
                error = %err,
                "dev agent wait failed during token refresh"
            );
        }

        handle.child = self.spawn_dev_agent_process(module_id, &handle.config_path)?;

        Ok(env_export.token.claims.expires_at)
    }

    pub fn issue_scoped_service_token(
        &self,
        module_id: &ModuleId,
        extra_scopes: Vec<ServiceScope>,
    ) -> Result<DelegatedToken, ModuleRuntimeError> {
        let mut merged = self.default_service_scopes.clone();
        for scope in extra_scopes {
            if !merged.iter().any(|existing| existing == &scope) {
                merged.push(scope);
            }
        }
        self.issue_service_token_with_scopes(module_id, merged)
    }

    fn issue_service_token_with_scopes(
        &self,
        module_id: &ModuleId,
        scopes: Vec<ServiceScope>,
    ) -> Result<DelegatedToken, ModuleRuntimeError> {
        let request = DelegatedTokenRequest::new(
            DelegatedActor::Service {
                service_id: Self::module_service_id(module_id),
                role: ServiceRole::Write,
            },
            DEFAULT_SERVICE_TENANT,
        )
        .with_scopes(scopes);
        self.security
            .issue_service_token(request)
            .map_err(|err| ModuleRuntimeError::InvalidState(err.to_string()))
    }

    pub(crate) async fn record_service_token(&self, module_id: &ModuleId, token: &DelegatedToken) {
        let lease = ModuleTokenLease::from(token);
        let mut guard = self.service_tokens.write().await;
        guard.insert(module_id.clone(), lease);
    }

    pub(super) fn audit_runtime_token_refresh(&self, module_id: &ModuleId, token: &DelegatedToken) {
        let Some(app_services) = self.app_services.upgrade() else {
            return;
        };
        let requested_scopes: &[String] = &[];
        record_module_token_exchange(
            app_services.as_ref(),
            module_id,
            ModuleTokenAuditContext {
                transport: "module-runtime",
                endpoint: None,
                requested_scopes,
                granted_scopes: Some(&token.claims.scopes),
                reason: Some("service_token_refresh"),
                expires_in_seconds: Self::token_ttl_seconds(token),
                outcome: AuditOutcome::Success,
                error: None,
            },
        );
    }

    fn token_ttl_seconds(token: &DelegatedToken) -> u64 {
        let now = OffsetDateTime::now_utc();
        if token.claims.expires_at <= now {
            return 0;
        }
        (token.claims.expires_at - now).whole_seconds().max(0) as u64
    }

    pub(super) fn record_lifecycle_metrics(&self, started_at: Instant, success: bool) {
        let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        self.diagnostics
            .record_probe("module-runtime", latency_ms, success);
        self.diagnostics
            .record_probe("module-lifecycle", latency_ms, success);
    }

    pub(super) async fn take_service_token(
        &self,
        module_id: &ModuleId,
    ) -> Option<ModuleTokenLease> {
        let mut guard = self.service_tokens.write().await;
        guard.remove(module_id)
    }

    pub(super) async fn revoke_service_token_if_any(&self, module_id: &ModuleId, reason: &str) {
        if let Some(lease) = self.take_service_token(module_id).await {
            if let Err(err) = self.security.revoke_service_token(lease.token(), reason) {
                tracing::warn!(
                    module = %module_id,
                    error = %err,
                    "{}",
                    module_service_logs::SERVICE_TOKEN_REVOKE_FAILED
                );
            }
        }
    }

    pub async fn token_leases_snapshot(&self) -> Vec<(ModuleId, ModuleTokenLease)> {
        let guard = self.service_tokens.read().await;
        guard
            .iter()
            .map(|(id, lease)| (id.clone(), lease.clone()))
            .collect()
    }

    pub async fn refresh_service_token(
        &self,
        module_id: &ModuleId,
    ) -> Result<ModuleTokenLease, ModuleRuntimeError> {
        let token = self.issue_default_service_token(module_id)?;
        self.record_service_token(module_id, &token).await;
        Ok(ModuleTokenLease::from(&token))
    }

    pub fn configure_db_connector(&self, endpoint: Option<DbConnectorEndpoint>) {
        match self.db_connector_endpoint.write() {
            Ok(mut guard) => {
                *guard = endpoint;
            }
            Err(_) => {
                tracing::warn!("{}", module_service_logs::DB_CONNECTOR_ENDPOINT_SET_FAILED);
            }
        }
    }

    pub async fn resolve_ingress_target(
        &self,
        service_id: &str,
    ) -> Result<ModuleIngressTarget, ModuleIngressError> {
        let Some(stripped) = service_id.strip_prefix("module:") else {
            return Err(ModuleIngressError::UnsupportedService(
                service_id.to_string(),
            ));
        };
        let (module_raw, suffix) = match stripped.split_once("::") {
            Some((module, svc)) => (module, Some(svc)),
            None => (stripped, None),
        };
        let module_id = ModuleId::new(module_raw)
            .map_err(|_| ModuleIngressError::InvalidModuleId(module_raw.to_string()))?;

        if suffix.is_some() {
            if let Some(target) = self.resolve_override_service(&module_id, service_id).await {
                return Ok(target);
            }
            if let Some(target) = self.resolve_declared_service(&module_id, service_id).await {
                return Ok(target);
            }
            if self
                .runtime_service_registered(&module_id, service_id)
                .await
            {
                return self.resolve_runtime_ingress_target(&module_id).await;
            }
            return Err(ModuleIngressError::DeclaredServiceMissing {
                module_id: module_id.to_string(),
                service_id: service_id.to_string(),
            });
        }

        if let Some(target) = self.resolve_first_override_service(&module_id).await {
            return Ok(target);
        }

        self.resolve_runtime_ingress_target(&module_id).await
    }

    pub fn new(init: ModuleServiceInit) -> Arc<Self> {
        Arc::new_cyclic(move |weak| {
            let ModuleServiceInit {
                registry,
                storage,
                verifier,
                runtime,
                service_registry,
                port_allocator,
                security,
                dev_sources,
                overrides,
                client_settings,
                health_client,
                default_service_scopes,
                env_passthrough_prefixes,
                control_plane_url,
                service_snapshot_path,
                diagnostics,
                services,
            } = init;
            let manifest_client = Client::builder()
                .timeout(client_settings.timeout)
                .build()
                .expect("module manifest client must build");
            let gateway_settings = GatewaySettings::from(&client_settings);
            let dev_sources_cfg = dev_sources.map(DevSourceConfig::new);
            Self {
                module_registry: registry,
                storage,
                verifier,
                runtime,
                service_registry,
                port_allocator,
                security,
                dev_sources: dev_sources_cfg,
                overrides: Arc::new(overrides),
                client_settings,
                health_client,
                diagnostics: Arc::clone(&diagnostics),
                runtime_gateways: Arc::new(RuntimeGatewayRegistry::new(
                    gateway_settings,
                    diagnostics,
                )),
                dev_overrides: Arc::new(RwLock::new(HashMap::new())),
                dev_agents: Arc::new(Mutex::new(HashMap::new())),
                dev_agent_rotations: Arc::new(Mutex::new(HashMap::new())),
                manual_token_rotations: Arc::new(Mutex::new(HashMap::new())),
                manifest_refresh_tasks: Arc::new(Mutex::new(HashMap::new())),
                declared_services: Arc::new(RwLock::new(HashMap::new())),
                runtime_services: Arc::new(RwLock::new(HashMap::new())),
                service_tokens: Arc::new(RwLock::new(HashMap::new())),
                db_connector_endpoint: SyncRwLock::new(None),
                default_service_scopes,
                env_passthrough_prefixes,
                control_plane_url: SyncRwLock::new(control_plane_url),
                health: SyncRwLock::new(HashMap::new()),
                manifest_client,
                service_snapshot_path,
                service_endpoints: Arc::new(RwLock::new(HashMap::new())),
                startup_reports: Arc::new(RwLock::new(HashMap::new())),
                app_services: services,
                self_ref: weak.clone(),
            }
        })
    }

    pub(super) fn module_service_id(module_id: &ModuleId) -> String {
        format!("module:{}", module_id)
    }

    pub(super) fn module_runtime_service_descriptor_id(
        module_id: &ModuleId,
        service_id: &str,
    ) -> String {
        format!("module:{}::{}", module_id, service_id)
    }

    pub(super) fn module_runtime_service_uri(module_id: &ModuleId, service_id: &str) -> String {
        format!(
            "service://{}",
            Self::module_runtime_service_descriptor_id(module_id, service_id)
        )
    }

    fn module_service_descriptor(
        module_id: &ModuleId,
        manifest: &ModuleManifest,
    ) -> ServiceDescriptorOwned {
        let service_id = Self::module_service_id(module_id);
        let name = manifest
            .title
            .clone()
            .unwrap_or_else(|| manifest.id.clone());
        let description = manifest
            .description
            .clone()
            .unwrap_or_else(|| format!("module {}", manifest.id));
        let ingress = ServiceIngressMetadata::internal().with_health_endpoint("/health");
        let security = ServiceSecurityMetadata::internal_default()
            .with_tenant_guard(ServiceTenantGuard::fixed(DEFAULT_SERVICE_TENANT));
        ServiceDescriptorOwned::new(service_id, name, description, ServiceKind::Other)
            .with_tags(vec![ServiceTag::Core])
            .with_security(security)
            .with_ingress(ingress)
    }

    pub(super) fn apply_descriptor_overrides(
        &self,
        mut descriptor: ServiceDescriptorOwned,
    ) -> ServiceDescriptorOwned {
        if let Some(policy) = self.overrides.security_override(descriptor.id()) {
            let base = descriptor
                .security
                .clone()
                .unwrap_or_else(ServiceSecurityMetadata::internal_default);
            descriptor.security = Some(policy.apply(base));
        }
        descriptor
    }

    pub(super) fn ensure_module_service_entry(
        &self,
        module_id: &ModuleId,
        manifest: &ModuleManifest,
    ) {
        let service_id = Self::module_service_id(module_id);
        if self.service_registry.get(&service_id).is_some() {
            return;
        }
        let descriptor =
            self.apply_descriptor_overrides(Self::module_service_descriptor(module_id, manifest));
        self.service_registry.register(
            descriptor,
            ServiceStatus::Standby,
            Some(module_service_notes::INSTALLED.to_string()),
        );
    }

    pub(super) fn update_module_service_status(
        &self,
        module_id: &ModuleId,
        manifest: &ModuleManifest,
        status: ServiceStatus,
        note: impl Into<Option<String>>,
    ) {
        self.ensure_module_service_entry(module_id, manifest);
        self.service_registry
            .set_status(&Self::module_service_id(module_id), status, note);
    }

    fn unregister_module_service(&self, module_id: &ModuleId) {
        self.service_registry
            .unregister(&Self::module_service_id(module_id));
    }

    pub(super) fn is_unmanaged_module(&self, module_id: &ModuleId) -> bool {
        let _ = module_id;
        false
    }

    pub(super) async fn is_dev_override_active(&self, module_id: &ModuleId) -> bool {
        self.dev_overrides.read().await.contains_key(module_id)
    }

    pub async fn dev_override_modules(&self) -> Vec<ModuleId> {
        self.dev_overrides.read().await.keys().cloned().collect()
    }

    async fn resolve_override_service(
        &self,
        module_id: &ModuleId,
        service_id: &str,
    ) -> Option<ModuleIngressTarget> {
        let guard = self.dev_overrides.read().await;
        guard.get(module_id).and_then(|state| {
            state.services.get(service_id).copied().map(|endpoint| {
                ModuleIngressTarget::DevService {
                    module_id: module_id.clone(),
                    service_id: service_id.to_string(),
                    endpoint,
                }
            })
        })
    }

    async fn resolve_first_override_service(
        &self,
        module_id: &ModuleId,
    ) -> Option<ModuleIngressTarget> {
        let guard = self.dev_overrides.read().await;
        guard.get(module_id).and_then(|state| {
            state
                .services
                .iter()
                .next()
                .map(|(svc_id, &endpoint)| ModuleIngressTarget::DevService {
                    module_id: module_id.clone(),
                    service_id: svc_id.clone(),
                    endpoint,
                })
        })
    }

    async fn resolve_declared_service(
        &self,
        module_id: &ModuleId,
        service_id: &str,
    ) -> Option<ModuleIngressTarget> {
        let guard = self.declared_services.read().await;
        guard.get(module_id).and_then(|state| {
            state.services.get(service_id).copied().map(|endpoint| {
                ModuleIngressTarget::DeclaredService {
                    module_id: module_id.clone(),
                    service_id: service_id.to_string(),
                    endpoint,
                }
            })
        })
    }

    async fn resolve_runtime_ingress_target(
        &self,
        module_id: &ModuleId,
    ) -> Result<ModuleIngressTarget, ModuleIngressError> {
        if self.is_dev_override_active(module_id).await {
            return Err(ModuleIngressError::ModuleNotRunning(module_id.to_string()));
        }
        match self.runtime.status(module_id).await {
            Ok(info) => {
                if !matches!(info.status, ModuleRuntimeStatus::Running) {
                    return Err(ModuleIngressError::ModuleNotRunning(module_id.to_string()));
                }
                if let Some(port) = info.port {
                    return Ok(ModuleIngressTarget::RuntimePort {
                        module_id: module_id.clone(),
                        port,
                    });
                }
                Err(ModuleIngressError::ModulePortUnknown(module_id.to_string()))
            }
            Err(ModuleRuntimeError::NotRunning { .. }) => {
                Err(ModuleIngressError::ModuleNotRunning(module_id.to_string()))
            }
            Err(err) => Err(ModuleIngressError::Runtime(err)),
        }
    }

    async fn runtime_service_registered(&self, module_id: &ModuleId, service_id: &str) -> bool {
        let guard = self.runtime_services.read().await;
        runtime_service_present(&guard, module_id, service_id)
    }

    pub(super) async fn ensure_distribution_backup(
        &self,
        module_id: &ModuleId,
        installed: &InstalledModule,
    ) -> ModuleResult<()> {
        if installed.source.is_synchronized()
            || Self::distribution_backup_exists(module_id, &installed.path).await
        {
            return Ok(());
        }
        self.write_distribution_backup(module_id, installed).await
    }

    async fn write_distribution_backup(
        &self,
        module_id: &ModuleId,
        installed: &InstalledModule,
    ) -> ModuleResult<()> {
        let module_dir = PathBuf::from(&installed.path);
        let backup_dir = Self::distribution_backup_dir(module_id, &module_dir);

        if tokio_fs::metadata(&backup_dir).await.is_ok() {
            let _ = tokio_fs::remove_dir_all(&backup_dir).await;
        }
        tokio_fs::create_dir_all(&backup_dir).await.map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
        })?;

        let archive = package_module_directory(module_dir.clone()).await?;
        let mut hasher = Sha256::new();
        hasher.update(&archive);
        let checksum = hasher.finalize().to_vec();

        let mut manifest = installed.manifest.clone();
        manifest.artifact.download_url = format!(
            "file://backup/{}/{}-backup.tar.gz",
            manifest.id, manifest.version
        );
        manifest.artifact.checksum.hash = hex::encode(&checksum);
        manifest.artifact.checksum.algorithm = ChecksumAlgorithm::Sha256;
        manifest.artifact.content_type = Some("application/gzip".to_string());
        manifest.artifact.size_bytes = Some(archive.len() as u64);

        let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
        })?;
        tokio_fs::write(backup_dir.join(BACKUP_MANIFEST_FILE), manifest_bytes)
            .await
            .map_err(|err| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
            })?;

        tokio_fs::write(backup_dir.join(BACKUP_ARCHIVE_FILE), &archive)
            .await
            .map_err(|err| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
            })?;

        tokio_fs::write(backup_dir.join(BACKUP_CHECKSUM_FILE), &checksum)
            .await
            .map_err(|err| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
            })?;

        tokio_fs::write(backup_dir.join(BACKUP_SIGNATURE_FILE), &[])
            .await
            .map_err(|err| {
                ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
            })?;

        Ok(())
    }

    async fn load_distribution_backup(
        &self,
        module_id: &ModuleId,
        module_path: &str,
    ) -> ModuleResult<Option<ModuleBundle>> {
        let dir = Self::distribution_backup_dir_from_str(module_id, module_path);
        if tokio_fs::metadata(&dir).await.is_err() {
            return Ok(None);
        }
        let manifest_path = dir.join(BACKUP_MANIFEST_FILE);
        let archive_path = dir.join(BACKUP_ARCHIVE_FILE);
        let signature_path = dir.join(BACKUP_SIGNATURE_FILE);
        let checksum_path = dir.join(BACKUP_CHECKSUM_FILE);
        if tokio_fs::metadata(&manifest_path).await.is_err()
            || tokio_fs::metadata(&archive_path).await.is_err()
        {
            return Ok(None);
        }

        let manifest_bytes = tokio_fs::read(&manifest_path).await.map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
        })?;
        let manifest: ModuleManifest = serde_json::from_slice(&manifest_bytes).map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
        })?;
        let archive = tokio_fs::read(&archive_path).await.map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
        })?;
        let signature = tokio_fs::read(&signature_path).await.unwrap_or_default();
        let checksum = tokio_fs::read(&checksum_path).await.unwrap_or_default();

        Ok(Some(ModuleBundle {
            manifest,
            archive,
            signature,
            checksum,
        }))
    }

    async fn clear_distribution_backup(
        &self,
        module_id: &ModuleId,
        module_path: &str,
    ) -> ModuleResult<()> {
        let dir = Self::distribution_backup_dir_from_str(module_id, module_path);
        if tokio_fs::metadata(&dir).await.is_err() {
            return Ok(());
        }
        tokio_fs::remove_dir_all(dir).await.map_err(|err| {
            ModuleServiceError::Storage(ModuleStorageError::InvalidState(err.to_string()))
        })
    }

    fn distribution_backup_dir(module_id: &ModuleId, module_dir: &Path) -> PathBuf {
        module_dir
            .parent()
            .map(|parent| parent.to_path_buf())
            .unwrap_or_else(|| module_dir.to_path_buf())
            .join(DISTRIBUTION_BACKUP_DIR)
            .join(module_id.as_str())
    }

    fn distribution_backup_dir_from_str(module_id: &ModuleId, module_path: &str) -> PathBuf {
        let module_dir = PathBuf::from(module_path);
        Self::distribution_backup_dir(module_id, module_dir.as_path())
    }

    async fn distribution_backup_exists(module_id: &ModuleId, module_path: &str) -> bool {
        let manifest_path = Self::distribution_backup_dir_from_str(module_id, module_path)
            .join(BACKUP_MANIFEST_FILE);
        tokio_fs::metadata(manifest_path).await.is_ok()
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
        self.install_internal(id, version, None, false).await
    }

    /// Install or update a module with progress callback
    pub async fn install_with_progress(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
        progress: Option<ProgressCallback>,
    ) -> ModuleResult<ModuleInstallResult> {
        self.install_internal(id, version, progress, false).await
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

    async fn install_internal(
        &self,
        id: &ModuleId,
        version: Option<&ModuleVersion>,
        progress: Option<ProgressCallback>,
        preserve_dev_overrides: bool,
    ) -> ModuleResult<ModuleInstallResult> {
        let manifest = self.module_registry.fetch_manifest(id, version).await?;
        let should_preserve = if preserve_dev_overrides {
            true
        } else {
            self.is_dev_override_active(id).await
        };
        if !should_preserve {
            self.clear_dev_services_if_any(id).await;
            self.clear_declared_services_if_any(id).await;
        }

        // Check if already installed
        let mut was_installed = false;
        if let Some(installed) = self.storage.load(id).await? {
            was_installed = true;
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

        if !should_preserve {
            if let Ok(module_id) = ModuleId::new(&result.manifest.id) {
                if !was_installed {
                    tracing::info!(
                        module = %module_id,
                        "fresh install detected, forcing runtime stop before start"
                    );
                    self.stop_module_process(&module_id).await;
                }
                self.update_module_service_status(
                    &module_id,
                    &result.manifest,
                    ServiceStatus::Standby,
                    Some(module_service_notes::INSTALLED.to_string()),
                );
                if !self.is_dev_override_active(&module_id).await {
                    if !was_installed {
                        tracing::info!(
                            module = %module_id,
                            "fresh install completed, starting module"
                        );
                    }
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
        let installed = match self.storage.load(id).await? {
            Some(installed) => installed,
            None => {
                return Err(ModuleServiceError::Storage(
                    ModuleStorageError::InvalidState(module_service_errors::module_not_installed(
                        id,
                    )),
                ))
            }
        };
        tracing::info!(module = %id, "uninstalling module");
        self.stop_module_process(id).await;
        if let Err(err) = cleanup_runtime_state(&installed.path, id).await {
            tracing::warn!(
                module = %id,
                error = %err,
                "failed to cleanup runtime state during uninstall"
            );
        }
        self.storage.remove(id).await?;
        if let Err(err) = self.clear_distribution_backup(id, &installed.path).await {
            tracing::warn!(
                module = %id,
                error = %err,
                "failed to remove distribution backup during uninstall"
            );
        }
        self.clear_dev_services_if_any(id).await;
        self.clear_declared_services_if_any(id).await;
        self.unregister_module_service(id);
        self.port_allocator.release(id).await;
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

            let preserve_dev_overrides = self.is_dev_override_active(&entry.module_id).await;
            let result = self
                .install_internal(
                    &entry.module_id,
                    Some(&entry.target_version),
                    None,
                    preserve_dev_overrides,
                )
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
    ) -> ModuleResult<ModuleReleaseOutcome> {
        self.release_override_with_restart(module_id, fenrir_version, true)
            .await
    }

    pub async fn release_override_without_restart(
        &self,
        module_id: &ModuleId,
        fenrir_version: &str,
    ) -> ModuleResult<ModuleReleaseOutcome> {
        self.release_override_with_restart(module_id, fenrir_version, false)
            .await
    }

    async fn release_override_with_restart(
        &self,
        module_id: &ModuleId,
        fenrir_version: &str,
        restart_after: bool,
    ) -> ModuleResult<ModuleReleaseOutcome> {
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

        let install_result = if let Some(bundle) = self
            .load_distribution_backup(module_id, &installed.path)
            .await?
        {
            let backup_version = bundle.manifest.module_version();
            match self
                .storage
                .stage_and_activate(bundle, ModuleInstallSource::Distribution)
                .await
            {
                Ok(result) => {
                    self.clear_distribution_backup(module_id, &installed.path)
                        .await?;
                    result
                }
                Err(err) => {
                    tracing::warn!(
                        module = %module_id,
                        error = %err,
                        "local distribution backup restore failed; attempting registry restore for same version"
                    );
                    self.clear_distribution_backup(module_id, &installed.path)
                        .await?;
                    match self.install(module_id, Some(&backup_version)).await {
                        Ok(result) => result,
                        Err(fallback_err) => {
                            tracing::warn!(
                                module = %module_id,
                                version = %backup_version,
                                error = %fallback_err,
                                "registry restore for backup version failed; falling back to distribution target"
                            );
                            self.install_from_registry(module_id, fenrir_version)
                                .await?
                        }
                    }
                }
            }
        } else {
            self.install_from_registry(module_id, fenrir_version)
                .await?
        };
        if restart_after {
            self.ensure_all_running().await;
        }
        Ok(ModuleReleaseOutcome {
            install_result,
            dev_override_cleared: dev_override_active,
        })
    }

    /// Release all modules that currently run with a dev override.
    pub async fn release_all_dev_overrides(
        &self,
        fenrir_version: &str,
    ) -> ModuleResult<Vec<(ModuleId, ModuleReleaseOutcome)>> {
        self.release_all_dev_overrides_with_restart(fenrir_version, true)
            .await
    }

    pub async fn release_all_dev_overrides_without_restart(
        &self,
        fenrir_version: &str,
    ) -> ModuleResult<Vec<(ModuleId, ModuleReleaseOutcome)>> {
        self.release_all_dev_overrides_with_restart(fenrir_version, false)
            .await
    }

    async fn release_all_dev_overrides_with_restart(
        &self,
        fenrir_version: &str,
        restart_after: bool,
    ) -> ModuleResult<Vec<(ModuleId, ModuleReleaseOutcome)>> {
        let modules = self.dev_override_modules().await;
        let mut outcomes = Vec::new();
        for module_id in modules {
            let outcome = self
                .release_override_with_restart(&module_id, fenrir_version, restart_after)
                .await?;
            outcomes.push((module_id, outcome));
        }
        Ok(outcomes)
    }

    async fn install_from_registry(
        &self,
        module_id: &ModuleId,
        fenrir_version: &str,
    ) -> ModuleResult<ModuleInstallResult> {
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
        self.install(module_id, Some(&target_version)).await
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

    pub(super) fn guard_quarantine(&self, module_id: &ModuleId) -> Result<(), ModuleRuntimeError> {
        if let Some(until) = self.quarantine_deadline(module_id) {
            return Err(ModuleRuntimeError::Quarantined {
                module_id: module_id.to_string(),
                resume_at: until,
            });
        }
        Ok(())
    }

    fn quarantine_deadline(&self, module_id: &ModuleId) -> Option<SystemTime> {
        let now = SystemTime::now();
        if let Ok(mut guard) = self.health.write() {
            if let Some(entry) = guard.get_mut(module_id) {
                if let Some(until) = entry.quarantined_until {
                    if now >= until {
                        entry.quarantined_until = None;
                        return None;
                    }
                    return Some(until);
                }
            }
        }
        None
    }

    pub(super) fn record_failure(&self, module_id: &ModuleId) -> Option<SystemTime> {
        let now = SystemTime::now();
        if let Ok(mut guard) = self.health.write() {
            let entry = guard.entry(module_id.clone()).or_default();
            if let Some(last) = entry.last_failure {
                if now.duration_since(last).unwrap_or(Duration::from_secs(0))
                    > Duration::from_secs(FAILURE_WINDOW_SECS)
                {
                    entry.failure_count = 0;
                }
            }
            entry.failure_count += 1;
            entry.last_failure = Some(now);
            if entry.failure_count >= FAILURE_THRESHOLD {
                entry.failure_count = 0;
                let until = now + Duration::from_secs(QUARANTINE_DURATION_SECS);
                entry.quarantined_until = Some(until);
                return Some(until);
            }
        }
        None
    }

    pub(super) fn clear_health(&self, module_id: &ModuleId) {
        if let Ok(mut guard) = self.health.write() {
            if let Some(entry) = guard.get_mut(module_id) {
                entry.failure_count = 0;
                entry.last_failure = None;
                entry.quarantined_until = None;
            }
        }
    }

    pub(super) fn annotate_quarantine(
        &self,
        module_id: &ModuleId,
        manifest: &ModuleManifest,
        until: SystemTime,
    ) {
        let formatted = system_time_to_rfc3339(until).unwrap_or_else(|| format!("{:?}", until));
        self.update_module_service_status(
            module_id,
            manifest,
            ServiceStatus::Failed,
            Some(module_service_notes::quarantined(formatted)),
        );
    }
}

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

fn runtime_service_present(
    registry: &std::collections::HashMap<ModuleId, Vec<String>>,
    module_id: &ModuleId,
    service_id: &str,
) -> bool {
    registry
        .get(module_id)
        .map(|services| services.iter().any(|id| id == service_id))
        .unwrap_or(false)
}

async fn cleanup_runtime_state(
    installed_path: &str,
    module_id: &ModuleId,
) -> Result<(), ModuleStorageError> {
    let module_path = PathBuf::from(installed_path);
    let Some(install_dir) = module_path.parent() else {
        return Ok(());
    };
    let state_path = install_dir.join("runtime").join("runtime-state.json");
    let Ok(contents) = tokio_fs::read_to_string(&state_path).await else {
        return Ok(());
    };
    let Ok(entries) = serde_json::from_str::<Vec<JsonValue>>(&contents) else {
        return Ok(());
    };
    let entries_len = entries.len();
    let filtered: Vec<JsonValue> = entries
        .into_iter()
        .filter(|entry| {
            entry
                .get("module_id")
                .and_then(|value| value.as_str())
                .map(|value| value != module_id.as_str())
                .unwrap_or(true)
        })
        .collect();
    if filtered.len() == entries_len {
        return Ok(());
    }
    let data = serde_json::to_vec_pretty(&filtered).map_err(|err| {
        ModuleStorageError::InvalidState(format!(
            "failed to serialize runtime state cleanup: {err}"
        ))
    })?;
    tokio_fs::write(&state_path, data).await.map_err(|err| {
        ModuleStorageError::Io(format!(
            "failed to write runtime state cleanup {}: {err}",
            state_path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_fenrir_compatibility, runtime_service_present};
    use crate::domain::module::{
        ChecksumAlgorithm, ModuleArtifactDescriptor, ModuleChecksum, ModuleId, ModuleManifest,
        ModuleSignatureDescriptor, SignatureAlgorithm,
    };
    use std::collections::HashMap;

    fn module(id: &str) -> ModuleId {
        ModuleId::new(id).expect("module id valid")
    }

    #[test]
    fn detects_registered_runtime_service() {
        let mut registry: HashMap<ModuleId, Vec<String>> = HashMap::new();
        let module = module("fenrir-api");
        registry.insert(
            module.clone(),
            vec![
                "module:fenrir-api::api-gateway".to_string(),
                "module:fenrir-api::health".to_string(),
            ],
        );

        assert!(runtime_service_present(
            &registry,
            &module,
            "module:fenrir-api::api-gateway"
        ));
        assert!(runtime_service_present(
            &registry,
            &module,
            "module:fenrir-api::health"
        ));
        assert!(!runtime_service_present(
            &registry,
            &module,
            "module:fenrir-api::missing"
        ));
    }

    #[test]
    fn returns_false_for_unknown_module() {
        let registry: HashMap<ModuleId, Vec<String>> = HashMap::new();
        let module = module("fenrir-api");
        assert!(!runtime_service_present(
            &registry,
            &module,
            "module:fenrir-api::api-gateway"
        ));
    }

    #[test]
    fn fenrir_version_compatibility() {
        let manifest = ModuleManifest {
            id: "m".to_string(),
            version: semver::Version::parse("1.0.0").unwrap(),
            title: None,
            description: None,
            fenrir_version: Some(semver::VersionReq::parse(">=1.0.0").unwrap()),
            authors: Vec::new(),
            license: None,
            artifact: ModuleArtifactDescriptor {
                download_url: "".to_string(),
                checksum: ModuleChecksum {
                    algorithm: ChecksumAlgorithm::Sha256,
                    hash: "".to_string(),
                },
                content_type: None,
                size_bytes: None,
            },
            signature: ModuleSignatureDescriptor {
                algorithm: SignatureAlgorithm::Ed25519,
                key_id: "".to_string(),
                signature: "".to_string(),
            },
            dependencies: Vec::new(),
            tags: Vec::new(),
            published_at: None,
        };
        assert!(check_fenrir_compatibility(&manifest, "1.2.3"));
        assert!(!check_fenrir_compatibility(&manifest, "0.9.0"));
    }
}
