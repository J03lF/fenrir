use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::audit::InMemoryAuditLog;
use crate::config::{self, AppConfig, ConfigError, ModuleRuntimeEngine};
use crate::domain::db::DbEngine;
use crate::domain::module::{ModuleId, ModuleVersion};
use crate::infra::http::{HttpServer, HTTP_SERVICE_ID};
use crate::infra::modules::{
    CompositeModuleRegistry, Ed25519ModuleVerifier, FilesystemModuleStorage, HttpModuleRegistry,
    InProcessModuleRuntime, LocalModuleRegistry, ProcessModuleRuntime,
};
use crate::infra::{db, logging, ssh, telemetry};
use crate::security::identity::build_identity_provider;
use crate::security::manager::{AuditSink, SecurityManager};
use crate::services::scheduler::install_default_jobs;
use crate::services::{
    AppServices, DbShellService, ModuleService, SchedulerService, ServiceDescriptor, ServiceKind,
    ServiceRegistry, ServiceStatus, ServiceTag, SessionService,
};
use anyhow::{anyhow, Result};
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::task;
use tracing::{debug, info, warn};

pub struct BootContext {
    pub config: Arc<AppConfig>,
    pub services: Arc<AppServices>,
    pub http_server: Arc<HttpServer>,
    pub logging: crate::infra::logging::ReloadHandle,
}

#[derive(Debug, Clone, Copy)]
pub enum BootErrorCode {
    ConfigLoad,
    ConfigInvalid,
    ConfigMissingSecret,
    LoggingInit,
    TelemetryInit,
    TelemetryProbe,
    DbAdapters,
    DbEngine,
    DbShellInit,
    AuditInit,
    ModuleRegistry,
    ModuleStorage,
    ModuleVerifier,
    ModuleAttach,
    IdentityInit,
    SecurityInit,
    SecurityAttach,
    SessionAttach,
    SchedulerJobs,
    HttpServerInit,
}

impl BootErrorCode {
    fn as_str(&self) -> &'static str {
        match self {
            BootErrorCode::ConfigLoad => "BOOT-CONFIG-LOAD",
            BootErrorCode::ConfigInvalid => "BOOT-CONFIG-INVALID",
            BootErrorCode::ConfigMissingSecret => "BOOT-CONFIG-MISSING-SECRET",
            BootErrorCode::LoggingInit => "BOOT-LOGGING-INIT",
            BootErrorCode::TelemetryInit => "BOOT-TELEMETRY-INIT",
            BootErrorCode::TelemetryProbe => "BOOT-TELEMETRY-PROBE",
            BootErrorCode::DbAdapters => "BOOT-DB-ADAPTERS",
            BootErrorCode::DbEngine => "BOOT-DB-ENGINE",
            BootErrorCode::DbShellInit => "BOOT-DB-SHELL",
            BootErrorCode::AuditInit => "BOOT-AUDIT",
            BootErrorCode::ModuleRegistry => "BOOT-MODULE-REGISTRY",
            BootErrorCode::ModuleStorage => "BOOT-MODULE-STORAGE",
            BootErrorCode::ModuleVerifier => "BOOT-MODULE-VERIFIER",
            BootErrorCode::ModuleAttach => "BOOT-MODULE-ATTACH",
            BootErrorCode::IdentityInit => "BOOT-IDENTITY-INIT",
            BootErrorCode::SecurityInit => "BOOT-SECURITY-INIT",
            BootErrorCode::SecurityAttach => "BOOT-SECURITY-ATTACH",
            BootErrorCode::SessionAttach => "BOOT-SESSION-ATTACH",
            BootErrorCode::SchedulerJobs => "BOOT-SCHEDULER-JOBS",
            BootErrorCode::HttpServerInit => "BOOT-HTTP-INIT",
        }
    }
}

impl fmt::Display for BootErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct BootError {
    code: BootErrorCode,
    message: &'static str,
    #[source]
    source: anyhow::Error,
}

impl BootError {
    fn new(code: BootErrorCode, message: &'static str, source: impl Into<anyhow::Error>) -> Self {
        Self {
            code,
            message,
            source: source.into(),
        }
    }

    fn from_config(err: ConfigError) -> Self {
        match err {
            ConfigError::MissingEnv { .. } => BootError::new(
                BootErrorCode::ConfigMissingSecret,
                "missing required secret",
                err,
            ),
            ConfigError::Invalid(_) => {
                BootError::new(BootErrorCode::ConfigInvalid, "configuration invalid", err)
            }
            ConfigError::MissingConfigFile { .. } => BootError::new(
                BootErrorCode::ConfigLoad,
                "configuration file not found",
                err,
            ),
            ConfigError::InvalidProfile { .. } => BootError::new(
                BootErrorCode::ConfigInvalid,
                "configuration profile invalid",
                err,
            ),
            ConfigError::Anyhow(_) => BootError::new(
                BootErrorCode::ConfigLoad,
                "failed to load configuration",
                err,
            ),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code.as_str()
    }

    pub fn message(&self) -> &'static str {
        self.message
    }
}

fn wrap_boot<T, E>(
    result: Result<T, E>,
    code: BootErrorCode,
    message: &'static str,
) -> Result<T, BootError>
where
    anyhow::Error: From<E>,
{
    result.map_err(|err| BootError::new(code, message, err))
}

pub fn boot() -> Result<BootContext, BootError> {
    let cfg = config::load().map_err(BootError::from_config)?;
    let runtime_dir = resolve_runtime_dir();
    env::set_var(
        "FENRIR_RUNTIME_DIR",
        runtime_dir.to_string_lossy().into_owned(),
    );
    let logging_handle = wrap_boot(
        logging::init_tracing(&cfg),
        BootErrorCode::LoggingInit,
        "failed to initialize logging",
    )?;
    wrap_boot(
        telemetry::init(&cfg),
        BootErrorCode::TelemetryInit,
        "failed to initialize telemetry",
    )?;

    let adapters = wrap_boot(
        db::manager::build_adapters(&cfg),
        BootErrorCode::DbAdapters,
        "failed to build database adapters",
    )?;
    let default_engine = wrap_boot(
        cfg.db.default_engine.parse::<DbEngine>(),
        BootErrorCode::DbEngine,
        "invalid default database engine",
    )?;
    let registry = Arc::new(ServiceRegistry::new());
    registry.register(
        ServiceDescriptor::new(
            "db-shell",
            "DB Shell Service",
            "Interaktive Datenbank-Subshell für Admin-Kommandos",
            ServiceKind::Infrastructure,
        )
        .with_tags(&[ServiceTag::Platform]),
        ServiceStatus::Active,
        Some("bereit".to_string()),
    );
    info!("core services registered in registry");
    registry.register(
        ServiceDescriptor::new(
            "ssh-server",
            "SSH Transport",
            "Secure Shell Zugang und interaktive Sitzungen",
            ServiceKind::Transport,
        )
        .with_tags(&[ServiceTag::Core, ServiceTag::Platform])
        .critical(),
        ServiceStatus::Starting,
        Some("Initialisierung".to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "cli-shell",
            "Lokale CLI",
            "Interaktive CLI-Shell (lokal)",
            ServiceKind::Cli,
        )
        .with_tags(&[ServiceTag::Auxiliary]),
        ServiceStatus::Standby,
        Some("Wartend auf Aufruf".to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "module-runtime",
            "Module Runtime",
            "Verwaltet installierte CLI-Module",
            ServiceKind::Infrastructure,
        )
        .with_tags(&[ServiceTag::Platform]),
        ServiceStatus::Standby,
        Some("Keine Module installiert".to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "scheduler",
            "Background Scheduler",
            "Verwaltet periodische Jobs und Tasks",
            ServiceKind::BackgroundJob,
        )
        .with_tags(&[ServiceTag::Core])
        .critical(),
        ServiceStatus::Starting,
        Some("Initialisierung".to_string()),
    );

    registry.register(
        ServiceDescriptor::new(
            HTTP_SERVICE_ID,
            "HTTP Transport",
            "REST-API, Health und Telemetrie",
            ServiceKind::Transport,
        )
        .with_tags(&[ServiceTag::Core]),
        if cfg.server.enable_http {
            ServiceStatus::Standby
        } else {
            ServiceStatus::Stopped
        },
        Some(if cfg.server.enable_http {
            "wartet auf Start".to_string()
        } else {
            "deaktiviert (enable_http=false)".to_string()
        }),
    );

    telemetry::attach_service_registry(Arc::clone(&registry));
    telemetry::start_system_metrics_sampler(&cfg);

    wrap_boot(
        crate::infra::telemetry::register_readiness_probe("services", {
            let registry = Arc::clone(&registry);
            move || {
                registry
                    .snapshot()
                    .into_iter()
                    .all(|svc| !matches!(svc.status, ServiceStatus::Failed))
            }
        }),
        BootErrorCode::TelemetryProbe,
        "failed to register telemetry readiness probe",
    )?;

    let db_shell_service = Arc::new(wrap_boot(
        DbShellService::new(default_engine, adapters),
        BootErrorCode::DbShellInit,
        "failed to initialize database shell service",
    )?);
    let scheduler_service = Arc::new(SchedulerService::new(Arc::clone(&registry)));
    scheduler_service.start();
    info!("scheduler service started");

    let audit_capacity = if cfg.audit.enabled {
        cfg.audit.buffer_capacity()
    } else {
        0
    };
    let audit_log: Arc<dyn crate::audit::AuditLog> = if audit_capacity == 0 {
        Arc::new(InMemoryAuditLog::new(audit_capacity))
    } else {
        let configured_path = cfg
            .audit
            .storage
            .path()
            .expect("audit storage path validated during configuration");
        let resolved_path = resolve_storage_path(&runtime_dir, configured_path);
        if let Some(parent) = resolved_path.parent() {
            wrap_boot(
                fs::create_dir_all(parent),
                BootErrorCode::AuditInit,
                "failed to prepare audit storage directory",
            )?;
        }
        let retention_hours = cfg.audit.storage.retention_hours().unwrap_or(24);
        let retention_seconds = retention_hours.checked_mul(3600).unwrap_or(u64::MAX);
        let persist_seconds = cfg.audit.storage.persist_interval_seconds().unwrap_or(30);
        let log_path = resolved_path.clone();
        let audit = wrap_boot(
            InMemoryAuditLog::with_persistence(
                audit_capacity,
                resolved_path,
                Duration::from_secs(retention_seconds),
                Duration::from_secs(persist_seconds),
            )
            .map_err(anyhow::Error::new),
            BootErrorCode::AuditInit,
            "failed to initialize audit persistence",
        )?;
        tracing::info!(
            path = %log_path.display(),
            capacity = audit_capacity,
            retention_hours,
            persist_seconds,
            "audit log persistence configured"
        );
        Arc::new(audit)
    };

    let remote_registry: Arc<dyn crate::domain::module::ModuleRegistryPort> = Arc::new(wrap_boot(
        HttpModuleRegistry::new(&cfg.modules.registry),
        BootErrorCode::ModuleRegistry,
        "failed to initialize module registry",
    )?);

    let mut registry_chain: Vec<Arc<dyn crate::domain::module::ModuleRegistryPort>> = Vec::new();
    if cfg.modules.registry.allow_offline {
        if let Some(local) = LocalModuleRegistry::try_new(&cfg.modules.registry) {
            let local_arc: Arc<dyn crate::domain::module::ModuleRegistryPort> = Arc::new(local);
            registry_chain.push(local_arc);
        }
    }
    registry_chain.push(Arc::clone(&remote_registry));

    let module_registry: Arc<dyn crate::domain::module::ModuleRegistryPort> =
        if registry_chain.len() == 1 {
            registry_chain
                .pop()
                .expect("registry_chain contains remote registry")
        } else {
            Arc::new(CompositeModuleRegistry::new(registry_chain))
        };
    let module_storage: Arc<dyn crate::domain::module::ModuleStoragePort> = Arc::new(wrap_boot(
        FilesystemModuleStorage::new(&cfg.modules.storage),
        BootErrorCode::ModuleStorage,
        "failed to initialize module storage",
    )?);
    let module_verifier: Arc<dyn crate::domain::module::ModuleVerifierPort> = Arc::new(wrap_boot(
        Ed25519ModuleVerifier::from_config(&cfg.modules.trust),
        BootErrorCode::ModuleVerifier,
        "failed to initialize module verifier",
    )?);
    let module_runtime: Arc<dyn crate::domain::module::ModuleRuntimePort> =
        match cfg.modules.runtime.engine {
            ModuleRuntimeEngine::Process => {
                let runtime_state_dir =
                    PathBuf::from(&cfg.modules.storage.install_dir).join("runtime");
                let runtime = Arc::new(ProcessModuleRuntime::new(
                    Arc::clone(&module_storage),
                    runtime_state_dir,
                ));

                if let Ok(handle) = Handle::try_current() {
                    let handle_clone = handle.clone();
                    let runtime_for_load = Arc::clone(&runtime);
                    let load_result = task::block_in_place(move || {
                        handle_clone.block_on(runtime_for_load.load_state())
                    });
                    if let Err(err) = load_result {
                        warn!(error = %err, "failed to restore module runtime state");
                    }
                } else {
                    warn!("tokio runtime not available, skipping module state restore");
                }

                runtime
            }
            ModuleRuntimeEngine::Stub => {
                Arc::new(InProcessModuleRuntime::new(Arc::clone(&module_storage)))
            }
        };
    let module_service = Arc::new(ModuleService::new(
        Arc::clone(&module_registry),
        Arc::clone(&module_storage),
        Arc::clone(&module_verifier),
        Arc::clone(&module_runtime),
    ));

    let services = Arc::new(AppServices::new(
        Arc::clone(&db_shell_service),
        Arc::clone(&scheduler_service),
        Arc::clone(&registry),
        Arc::clone(&audit_log),
    ));
    services.set_logging_handle(logging_handle.clone());
    services
        .attach_module_service(Arc::clone(&module_service))
        .map_err(|err| {
            BootError::new(
                BootErrorCode::ModuleAttach,
                "failed to attach module service",
                anyhow!(err),
            )
        })?;
    let audit_sink: Arc<dyn AuditSink> = Arc::clone(&services) as Arc<dyn AuditSink>;
    let identity_service = wrap_boot(
        build_identity_provider(&cfg, runtime_dir.as_path(), Arc::clone(&audit_sink)),
        BootErrorCode::IdentityInit,
        "failed to initialize identity provider",
    )?;
    services
        .attach_identity(Arc::clone(&identity_service))
        .map_err(|err| {
            BootError::new(
                BootErrorCode::IdentityInit,
                "failed to attach identity service",
                anyhow!(err),
            )
        })?;
    registry.register(
        ServiceDescriptor::new(
            "identity-service",
            "Identity Broker",
            "Ausstellung und Prüfung von Control-Plane-Token",
            ServiceKind::Security,
        )
        .with_tags(&[ServiceTag::Core]),
        ServiceStatus::Active,
        Some("bereit".to_string()),
    );
    let _security_manager = Arc::new(wrap_boot(
        SecurityManager::new(&cfg.security, audit_sink),
        BootErrorCode::SecurityInit,
        "failed to initialize security manager",
    )?);
    services
        .attach_security(Arc::clone(&_security_manager))
        .map_err(|err| {
            BootError::new(
                BootErrorCode::SecurityAttach,
                "failed to attach security manager",
                anyhow!(err),
            )
        })?;
    let session_service = Arc::new(SessionService::new(Arc::clone(&_security_manager)));
    services
        .attach_session(Arc::clone(&session_service))
        .map_err(|err| {
            BootError::new(
                BootErrorCode::SessionAttach,
                "failed to attach session service",
                anyhow!(err),
            )
        })?;
    registry.set_status(
        "module-runtime",
        ServiceStatus::Active,
        Some("Bereit für Module".to_string()),
    );

    wrap_boot(
        install_default_jobs(
            &scheduler_service,
            Arc::clone(&registry),
            Arc::clone(&db_shell_service),
        ),
        BootErrorCode::SchedulerJobs,
        "failed to install scheduler jobs",
    )?;

    let http_server = Arc::new(wrap_boot(
        HttpServer::new(&cfg, Arc::clone(&registry), Arc::downgrade(&services)),
        BootErrorCode::HttpServerInit,
        "failed to initialize http server",
    )?);
    {
        let http_start = Arc::clone(&http_server);
        let http_stop = Arc::clone(&http_server);
        services.register_dynamic_service(
            HTTP_SERVICE_ID,
            move || {
                let server = Arc::clone(&http_start);
                Box::pin(async move { HttpServer::start(&server).await })
            },
            move |force| {
                let server = Arc::clone(&http_stop);
                Box::pin(async move { HttpServer::stop(&server, force).await })
            },
        );
    }
    {
        let scheduler_for_start = Arc::clone(&scheduler_service);
        let scheduler_for_stop = Arc::clone(&scheduler_service);
        let registry_for_jobs = Arc::clone(&registry);
        let db_shell_for_jobs = Arc::clone(&db_shell_service);
        services.register_dynamic_service(
            "scheduler",
            move || {
                let scheduler = Arc::clone(&scheduler_for_start);
                let registry = Arc::clone(&registry_for_jobs);
                let db_shell = Arc::clone(&db_shell_for_jobs);
                Box::pin(async move {
                    if scheduler.start() {
                        install_default_jobs(&scheduler, registry, db_shell)
                            .map_err(|err| anyhow!(err))?;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })
            },
            move |_force| {
                let scheduler = Arc::clone(&scheduler_for_stop);
                Box::pin(async move { Ok(scheduler.stop()) })
            },
        );
    }
    {
        let db_shell_start = Arc::clone(&db_shell_service);
        let db_shell_stop = Arc::clone(&db_shell_service);
        let registry_for_db_shell = Arc::clone(&registry);
        let registry_for_db_shell_stop = Arc::clone(&registry);
        services.register_dynamic_service(
            "db-shell",
            move || {
                let service = Arc::clone(&db_shell_start);
                let registry = Arc::clone(&registry_for_db_shell);
                Box::pin(async move {
                    if service.is_enabled() {
                        Ok(false)
                    } else {
                        service.set_enabled(true);
                        registry.set_status(
                            "db-shell",
                            ServiceStatus::Active,
                            Some("DB-Shell aktiviert".to_string()),
                        );
                        Ok(true)
                    }
                })
            },
            move |_force| {
                let service = Arc::clone(&db_shell_stop);
                let registry = Arc::clone(&registry_for_db_shell_stop);
                Box::pin(async move {
                    if !service.is_enabled() {
                        Ok(false)
                    } else {
                        service.set_enabled(false);
                        registry.set_status(
                            "db-shell",
                            ServiceStatus::Standby,
                            Some("DB-Shell deaktiviert".to_string()),
                        );
                        Ok(true)
                    }
                })
            },
        );
    }
    register_registry_toggle_service(
        &services,
        &registry,
        "cli-shell",
        ServiceStatus::Standby,
        "Bereit für neue Sessions",
        ServiceStatus::Stopped,
        "CLI deaktiviert",
    );

    crate::infra::telemetry::mark_ready();

    tracing::info!(app = %cfg.app.name, version = %cfg.app.version, "boot complete");
    Ok(BootContext {
        config: Arc::new(cfg),
        services,
        http_server,
        logging: logging_handle,
    })
}

pub async fn start_transports(ctx: &BootContext) -> Result<()> {
    if !ctx.config.modules.bootstrap.is_empty() {
        match ctx.services.module_service() {
            Some(module_service) => {
                bootstrap_modules(Arc::clone(&ctx.config), module_service).await;
            }
            None => {
                warn!("module service not attached; skipping bootstrap modules");
            }
        }
    }

    // Start SSH server (blocking future) on its own task
    let cfg = Arc::clone(&ctx.config);
    let services = Arc::clone(&ctx.services);
    tokio::spawn(async move {
        services.registry().set_status(
            "ssh-server",
            ServiceStatus::Starting,
            Some("Starte Listener".to_string()),
        );
        info!("ssh server task spawned");
        if let Err(e) = ssh::start(&cfg, &services).await {
            tracing::error!(error=%e, "ssh server exited with error");
            services.registry().set_status(
                "ssh-server",
                ServiceStatus::Failed,
                Some(format!("Fehler: {e}")),
            );
        }
    });

    if ctx.config.server.enable_http {
        let registry = ctx.services.registry();
        registry.set_status(
            HTTP_SERVICE_ID,
            ServiceStatus::Starting,
            Some("initialisiere".to_string()),
        );
        let http_server = Arc::clone(&ctx.http_server);
        tokio::spawn(async move {
            if let Err(err) = http_server.start().await {
                tracing::error!(error = %err, "http server start failed");
                registry.set_status(
                    HTTP_SERVICE_ID,
                    ServiceStatus::Failed,
                    Some(format!("Start-Fehler: {err}")),
                );
            }
        });
    }
    spawn_config_reloader(ctx)?;
    info!("transport initialisation triggered");
    Ok(())
}

async fn bootstrap_modules(config: Arc<AppConfig>, service: Arc<ModuleService>) {
    for entry in &config.modules.bootstrap {
        let module_spec = entry.trim();
        if module_spec.is_empty() {
            continue;
        }
        let (module_id, version) = match parse_bootstrap_spec(module_spec) {
            Ok(spec) => spec,
            Err(err) => {
                warn!(module = %module_spec, error = %err, "invalid bootstrap module spec");
                continue;
            }
        };

        match service.install(&module_id, version.as_ref()).await {
            Ok(result) => {
                info!(
                    module = %module_id,
                    version = version
                        .as_ref()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "latest".to_string()),
                    status = ?result.status,
                    "bootstrap module ready"
                );
            }
            Err(err) => {
                warn!(module = %module_id, error = %err, "bootstrap module install failed");
            }
        }
    }
}

fn parse_bootstrap_spec(spec: &str) -> Result<(ModuleId, Option<ModuleVersion>), String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err("empty module spec".to_string());
    }

    let (id_part, version_part) = match trimmed.split_once('@') {
        Some((_id, version)) if version.trim().is_empty() => {
            return Err("empty version segment".to_string());
        }
        Some((id, version)) => (id, Some(version.trim())),
        None => (trimmed, None),
    };

    let module_id = ModuleId::new(id_part.trim()).map_err(|err| err.to_string())?;
    let version = if let Some(version_raw) = version_part {
        Some(ModuleVersion::parse(version_raw).map_err(|err| err.to_string())?)
    } else {
        None
    };

    Ok((module_id, version))
}

#[cfg(test)]
mod tests {
    use super::parse_bootstrap_spec;
    use crate::domain::module::ModuleId;

    #[test]
    fn parses_module_without_version() {
        let expected_id = ModuleId::new("fenrir-api").unwrap();
        let (id, version) = parse_bootstrap_spec("fenrir-api").expect("spec parses");

        assert_eq!(id, expected_id);
        assert!(version.is_none());
    }

    #[test]
    fn parses_module_with_version() {
        let (id, version) = parse_bootstrap_spec("fenrir-api@1.2.3").expect("spec parses");

        assert_eq!(id, ModuleId::new("fenrir-api").unwrap());
        let version = version.expect("version present");
        assert_eq!(version.to_string(), "1.2.3");
    }

    #[test]
    fn rejects_missing_version_segment() {
        assert!(parse_bootstrap_spec("fenrir-api@").is_err());
    }

    #[test]
    fn rejects_invalid_version() {
        assert!(parse_bootstrap_spec("fenrir-api@not-a-version").is_err());
    }
}

fn resolve_runtime_dir() -> PathBuf {
    if let Ok(dir) = env::var("FENRIR_RUNTIME_DIR") {
        let candidate = PathBuf::from(dir);
        if ensure_dir(&candidate) {
            return candidate;
        }
    }

    let mut candidates = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join("tmp"));
        }
        candidates.push(cwd.join("tmp"));
    }

    for candidate in candidates {
        if ensure_dir(&candidate) {
            return candidate;
        } else {
            debug!(path = ?candidate, "failed to create runtime directory candidate");
        }
    }

    let fallback = env::temp_dir().join("fenrir-runtime");
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn resolve_storage_path(base: &Path, configured: &str) -> PathBuf {
    let candidate = PathBuf::from(configured);
    if candidate.is_absolute() {
        candidate
    } else {
        base.join(configured)
    }
}

fn ensure_dir(path: &Path) -> bool {
    fs::create_dir_all(path).is_ok()
}

fn register_registry_toggle_service(
    services: &AppServices,
    registry: &Arc<ServiceRegistry>,
    id: &'static str,
    active_status: ServiceStatus,
    active_note: &'static str,
    standby_status: ServiceStatus,
    standby_note: &'static str,
) {
    let state = Arc::new(AtomicBool::new(true));
    let registry_for_start = Arc::clone(registry);
    let registry_for_stop = Arc::clone(registry);
    services.register_dynamic_service(
        id,
        {
            let state = Arc::clone(&state);
            move || {
                let registry = Arc::clone(&registry_for_start);
                let state = Arc::clone(&state);
                Box::pin(async move {
                    let was_active = state.swap(true, Ordering::SeqCst);
                    if was_active {
                        Ok(false)
                    } else {
                        registry.set_status(id, active_status, Some(active_note.to_string()));
                        Ok(true)
                    }
                })
            }
        },
        {
            let state = Arc::clone(&state);
            move |_force| {
                let registry = Arc::clone(&registry_for_stop);
                let state = Arc::clone(&state);
                Box::pin(async move {
                    let was_active = state.swap(false, Ordering::SeqCst);
                    if !was_active {
                        Ok(false)
                    } else {
                        registry.set_status(id, standby_status, Some(standby_note.to_string()));
                        Ok(true)
                    }
                })
            }
        },
    );
}

fn spawn_config_reloader(ctx: &BootContext) -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let logging = ctx.logging.clone();
        let http_server = Arc::clone(&ctx.http_server);
        tokio::spawn(async move {
            let mut hup = match signal(SignalKind::hangup()) {
                Ok(signal) => signal,
                Err(err) => {
                    tracing::warn!(error = %err, "konnte SIGHUP-Signal-Handler nicht initialisieren");
                    return;
                }
            };

            while hup.recv().await.is_some() {
                match crate::config::load() {
                    Ok(new_cfg) => {
                        if let Err(err) =
                            logging::reload(&logging, &new_cfg.telemetry.tracing.level)
                        {
                            tracing::warn!(error = %err, "konnte Logging-Level nicht aktualisieren");
                        }
                        telemetry::reload(&new_cfg);
                        if let Err(err) = http_server.reload_tls(&new_cfg.server.http.tls).await {
                            tracing::warn!(error = %err, "TLS-Reload fehlgeschlagen");
                        }
                        tracing::info!("Konfiguration (Logging/Telemetry/TLS) neu geladen");
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "Konfigurations-Reload fehlgeschlagen")
                    }
                }
            }
        });
    }

    #[cfg(not(unix))]
    {
        tracing::info!(
            "Config-Hot-Reload wird auf dieser Plattform nicht unterstützt; SIGHUP-Replay übersprungen"
        );
    }

    Ok(())
}
