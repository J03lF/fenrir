use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use tokio::runtime::Handle;
use tokio::task;
use tracing::{info, warn};

use crate::audit::{AuditLog, InMemoryAuditLog};
use crate::config::{self, DbRuntimeMode, ModuleRuntimeEngine};
use crate::domain::db::DbEngine;
use crate::infra::db::runtime::DbRuntimeSupervisor;
use crate::infra::http::{HttpServer, HTTP_SERVICE_ID};
use crate::infra::modules::{
    CompositeModuleRegistry, Ed25519ModuleVerifier, FilesystemModuleStorage, HttpModuleRegistry,
    InProcessModuleRuntime, LocalModuleRegistry, ProcessModuleRuntime,
};
use crate::infra::{db, logging, telemetry};
use crate::security::identity::build_identity_provider_with_db;
use crate::security::manager::{AuditSink, SecurityManager};
use crate::security::service::ServiceScope;
use crate::services::scheduler::{install_default_jobs, SchedulerJobContext};
use crate::services::{
    block_on_managed, AppServices, DbShellService, ManagedService, ModuleClientSettings,
    ModuleHealthHttpClient, ModulePortAllocator, ModuleService, ModuleServiceInit,
    ModuleServiceOverrides, SchedulerService, ServiceDescriptor, ServiceDiagnostics, ServiceKind,
    ServiceRegistry, ServiceStatus, ServiceTag, SessionService, TokenExchangeService,
};
use crate::utils::messages::boot::{
    errors as boot_errors, logs as boot_logs, runtime as runtime_messages,
    services::{
        descriptions as service_descriptions, names as service_names, notes as service_notes,
    },
};

use super::context::BootContext;
use super::error::{wrap_boot, BootError, BootErrorCode};
use super::helpers::resolve_storage_path;
use super::registry::register_registry_toggle_service;

pub fn boot() -> Result<BootContext, BootError> {
    let cfg = config::load().map_err(BootError::from_config)?;
    // Use config-based runtime paths (FENRIR_RUNTIME_DIR still takes precedence)
    let runtime_dir = cfg.runtime.resolve_base_path();
    // Ensure runtime directory exists
    if let Err(e) = std::fs::create_dir_all(&runtime_dir) {
        tracing::warn!(error = %e, path = %runtime_dir.display(), "failed to create runtime directory");
    }
    env::set_var(
        "FENRIR_RUNTIME_DIR",
        runtime_dir.to_string_lossy().into_owned(),
    );
    let logging_handle = wrap_boot(
        logging::init_tracing(&cfg),
        BootErrorCode::LoggingInit,
        boot_errors::LOGGING_INIT_FAILED,
    )?;
    wrap_boot(
        telemetry::init(&cfg),
        BootErrorCode::TelemetryInit,
        boot_errors::TELEMETRY_INIT_FAILED,
    )?;

    // Early SecurityManager for DB credential sealing (before real audit store exists).
    // Uses NoopAuditSink since credential operations aren't audit-critical.
    let early_security_manager: Option<Arc<SecurityManager>> = if cfg.db.runtime.mode
        == DbRuntimeMode::Embedded
        && cfg
            .db
            .runtime
            .embedded
            .security
            .auth_method
            .requires_password()
    {
        use crate::security::manager::NoopAuditSink;
        let noop_audit = Arc::new(NoopAuditSink);
        Some(Arc::new(wrap_boot(
            SecurityManager::new(&cfg.security, noop_audit),
            BootErrorCode::SecurityInit,
            boot_errors::SECURITY_MANAGER_INIT_FAILED,
        )?))
    } else {
        None
    };

    // Early supervisor creation for embedded postgres (needs to start before adapters are built).
    let early_db_runtime: Option<Arc<DbRuntimeSupervisor>> =
        if cfg.db.runtime.mode == DbRuntimeMode::Embedded {
            let db_dir = cfg.runtime.db_path();
            let sup = DbRuntimeSupervisor::new_early(
                cfg.db.runtime.mode,
                cfg.db.runtime.embedded.engine,
                cfg.db.runtime.embedded.postgres.port_range,
                cfg.db.runtime.embedded.postgres.binary_path.clone(),
                db_dir,
                cfg.db.runtime.embedded.security.clone(),
            );

            // Attach early security manager if password auth is required
            if let Some(ref security) = early_security_manager {
                sup.attach_security(Arc::clone(security));
            }

            // Start the supervisor (blocking) to get connector URI before building adapters.
            let sup_clone = Arc::clone(&sup);
            let start_result = block_on_managed(async move { sup_clone.start().await });
            if let Err(err) = start_result {
                return Err(BootError::new(
                    BootErrorCode::DbRuntimeStart,
                    boot_errors::DB_RUNTIME_START_FAILED,
                    err,
                ));
            }
            Some(sup)
        } else {
            None
        };

    // Build runtime override from early supervisor if available.
    let runtime_override = early_db_runtime.as_ref().and_then(|sup| {
        sup.connector_uri().map(|uri| {
            use crate::config::EmbeddedEngineKind;
            let engine = match cfg.db.runtime.embedded.engine {
                EmbeddedEngineKind::Sqlite => DbEngine::Sqlite,
                EmbeddedEngineKind::Postgres => DbEngine::Postgres,
            };
            db::manager::RuntimeDbOverride { engine, uri }
        })
    });

    let adapters = wrap_boot(
        db::manager::build_adapters(&cfg, runtime_override),
        BootErrorCode::DbAdapters,
        boot_errors::DB_ADAPTERS_FAILED,
    )?;
    let default_engine = wrap_boot(
        cfg.db.default_engine.parse::<DbEngine>(),
        BootErrorCode::DbEngine,
        boot_errors::DEFAULT_DB_ENGINE_INVALID,
    )?;
    let registry = Arc::new(ServiceRegistry::new());
    registry.register(
        ServiceDescriptor::new(
            "db-shell",
            service_names::DB_SHELL,
            service_descriptions::DB_SHELL,
            ServiceKind::Infrastructure,
        )
        .with_tags(&[ServiceTag::Platform]),
        ServiceStatus::Active,
        Some(service_notes::READY.to_string()),
    );
    info!("{}", boot_logs::CORE_SERVICES_REGISTERED);
    registry.register(
        ServiceDescriptor::new(
            "ssh-server",
            service_names::SSH,
            service_descriptions::SSH,
            ServiceKind::Transport,
        )
        .with_tags(&[ServiceTag::Core, ServiceTag::Platform])
        .critical(),
        ServiceStatus::Starting,
        Some(service_notes::INITIALIZATION.to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "cli-shell",
            service_names::CLI,
            service_descriptions::CLI,
            ServiceKind::Cli,
        )
        .with_tags(&[ServiceTag::Auxiliary]),
        ServiceStatus::Standby,
        Some(service_notes::WAITING_FOR_INVOKE.to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "module-runtime",
            service_names::MODULE_RUNTIME,
            service_descriptions::MODULE_RUNTIME,
            ServiceKind::Infrastructure,
        )
        .with_tags(&[ServiceTag::Core, ServiceTag::Platform])
        .critical(),
        ServiceStatus::Standby,
        Some(service_notes::NO_MODULES.to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "scheduler",
            service_names::SCHEDULER,
            service_descriptions::SCHEDULER,
            ServiceKind::BackgroundJob,
        )
        .with_tags(&[ServiceTag::Core])
        .critical(),
        ServiceStatus::Starting,
        Some(service_notes::INITIALIZATION.to_string()),
    );

    registry.register(
        ServiceDescriptor::new(
            HTTP_SERVICE_ID,
            service_names::HTTP,
            service_descriptions::HTTP,
            ServiceKind::Transport,
        )
        .with_tags(&[ServiceTag::Core]),
        if cfg.server.enable_http {
            ServiceStatus::Standby
        } else {
            ServiceStatus::Stopped
        },
        Some(if cfg.server.enable_http {
            service_notes::HTTP_WAITING.to_string()
        } else {
            service_notes::HTTP_DISABLED.to_string()
        }),
    );

    registry.register(
        ServiceDescriptor::new(
            "token-exchange",
            service_names::TOKEN_EXCHANGE,
            service_descriptions::TOKEN_EXCHANGE,
            ServiceKind::Security,
        )
        .with_tags(&[ServiceTag::Core]),
        ServiceStatus::Standby,
        Some(service_notes::INITIALIZATION.to_string()),
    );

    registry.register(
        ServiceDescriptor::new(
            "module-lifecycle",
            service_names::MODULE_LIFECYCLE,
            service_descriptions::MODULE_LIFECYCLE,
            ServiceKind::Infrastructure,
        )
        .with_tags(&[ServiceTag::Platform]),
        ServiceStatus::Standby,
        Some(service_notes::INITIALIZATION.to_string()),
    );

    registry.register(
        ServiceDescriptor::new(
            "jobs-control",
            service_names::JOBS_CONTROL,
            service_descriptions::JOBS_CONTROL,
            ServiceKind::BackgroundJob,
        )
        .with_tags(&[ServiceTag::Platform]),
        ServiceStatus::Standby,
        Some(service_notes::INITIALIZATION.to_string()),
    );
    if cfg.db.runtime.mode == DbRuntimeMode::Embedded {
        registry.register(
            ServiceDescriptor::new(
                "db-runtime",
                service_names::DB_RUNTIME,
                service_descriptions::DB_RUNTIME,
                ServiceKind::Infrastructure,
            )
            .with_tags(&[ServiceTag::Platform]),
            ServiceStatus::Standby,
            Some(service_notes::INITIALIZATION.to_string()),
        );
    }

    let diagnostics = Arc::new(ServiceDiagnostics::new());

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
        boot_errors::TELEMETRY_PROBE_FAILED,
    )?;

    // When in embedded mode, override default_engine to match the embedded engine.
    let effective_default_engine = if cfg.db.runtime.mode == DbRuntimeMode::Embedded {
        use crate::config::EmbeddedEngineKind;
        match cfg.db.runtime.embedded.engine {
            EmbeddedEngineKind::Sqlite => DbEngine::Sqlite,
            EmbeddedEngineKind::Postgres => DbEngine::Postgres,
        }
    } else {
        default_engine
    };

    let db_shell_service = Arc::new(wrap_boot(
        DbShellService::new(effective_default_engine, adapters),
        BootErrorCode::DbShellInit,
        boot_errors::DB_SHELL_INIT_FAILED,
    )?);
    let migration_report = wrap_boot(
        block_on_managed(db::migrations::apply_pending_migrations(
            runtime_dir.clone(),
            Arc::clone(&db_shell_service),
            effective_default_engine,
        )),
        BootErrorCode::DbMigrations,
        boot_errors::DB_MIGRATIONS_FAILED,
    )?;
    if migration_report.applied_count() > 0 {
        info!(
            engine = %effective_default_engine,
            applied = migration_report.applied_count(),
            files = ?migration_report.applied(),
            "database migrations applied"
        );
    } else {
        info!(engine = %effective_default_engine, "database migrations up to date");
    }
    let scheduler_state_dir = cfg.runtime.scheduler_path();
    let scheduler_service = Arc::new(SchedulerService::new(
        Arc::clone(&registry),
        Arc::clone(&diagnostics),
        scheduler_state_dir,
    ));
    scheduler_service.start();
    info!("{}", boot_logs::SCHEDULER_STARTED);

    let audit_capacity = if cfg.audit.enabled {
        cfg.audit.buffer_capacity()
    } else {
        0
    };
    let audit_log: Arc<dyn AuditLog> = if audit_capacity == 0 {
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
                boot_errors::AUDIT_DIR_PREP_FAILED,
            )?;
        }
        let retention_hours = cfg.audit.storage.retention_hours().unwrap_or(48);
        let retention_seconds = retention_hours.saturating_mul(3600);
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
            boot_errors::AUDIT_PERSISTENCE_INIT_FAILED,
        )?;
        tracing::info!(
            path = %log_path.display(),
            capacity = audit_capacity,
            retention_hours,
            persist_seconds,
            "{}",
            boot_logs::AUDIT_PERSISTENCE_CONFIGURED
        );
        Arc::new(audit)
    };

    let services = Arc::new(AppServices::new(
        Arc::clone(&db_shell_service),
        Arc::clone(&scheduler_service),
        Arc::clone(&registry),
        Arc::clone(&audit_log),
        Arc::clone(&diagnostics),
    ));
    services.set_logging_handle(logging_handle.clone());
    let audit_sink: Arc<dyn AuditSink> = Arc::clone(&services) as Arc<dyn AuditSink>;

    let remote_registry: Arc<dyn crate::domain::module::ModuleRegistryPort> = Arc::new(wrap_boot(
        HttpModuleRegistry::new(&cfg.modules.registry),
        BootErrorCode::ModuleRegistry,
        boot_errors::MODULE_REGISTRY_INIT_FAILED,
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
        boot_errors::MODULE_STORAGE_INIT_FAILED,
    )?);
    let module_verifier: Arc<dyn crate::domain::module::ModuleVerifierPort> = Arc::new(wrap_boot(
        Ed25519ModuleVerifier::from_config(&cfg.modules.trust),
        BootErrorCode::ModuleVerifier,
        boot_errors::MODULE_VERIFIER_INIT_FAILED,
    )?);
    let runtime_state_dir = PathBuf::from(&cfg.modules.storage.install_dir).join("runtime");
    let module_runtime: Arc<dyn crate::domain::module::ModuleRuntimePort> =
        match cfg.modules.runtime.engine {
            ModuleRuntimeEngine::Process => {
                let control_plane_base = control_plane_base_url(&cfg.server.http);
                let runtime = Arc::new(ProcessModuleRuntime::new(
                    Arc::clone(&module_storage),
                    runtime_state_dir.clone(),
                    Arc::clone(&diagnostics),
                    control_plane_base,
                ));

                if let Ok(handle) = Handle::try_current() {
                    let handle_clone = handle.clone();
                    let runtime_for_load = Arc::clone(&runtime);
                    let load_result = task::block_in_place(move || {
                        handle_clone.block_on(runtime_for_load.load_state())
                    });
                    if let Err(err) = load_result {
                        warn!(
                            error = %err,
                            "{}",
                            runtime_messages::STATE_RESTORE_FAILED
                        );
                    }
                } else {
                    warn!("{}", runtime_messages::HANDLE_MISSING);
                }

                runtime
            }
            ModuleRuntimeEngine::Stub => {
                Arc::new(InProcessModuleRuntime::new(Arc::clone(&module_storage)))
            }
        };
    let port_allocator = Arc::new(ModulePortAllocator::new(
        cfg.modules.runtime.ports.strategy,
        cfg.modules.runtime.ports.range,
        runtime_state_dir.join("ports.json"),
    ));
    let dev_sources = cfg
        .modules
        .dev_sources
        .base_path
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    // defer module_service creation until after security manager is ready
    let identity_dir = cfg.runtime.identity_path();
    let identity_service = wrap_boot(
        build_identity_provider_with_db(
            &cfg,
            identity_dir.as_path(),
            Some(Arc::clone(&db_shell_service)),
            Arc::clone(&audit_sink),
        ),
        BootErrorCode::IdentityInit,
        boot_errors::IDENTITY_PROVIDER_INIT_FAILED,
    )?;
    services
        .attach_identity(Arc::clone(&identity_service))
        .map_err(|err| {
            BootError::new(
                BootErrorCode::IdentityInit,
                boot_errors::IDENTITY_SERVICE_ATTACH_FAILED,
                anyhow!(err),
            )
        })?;
    registry.register(
        ServiceDescriptor::new(
            "identity-service",
            service_names::IDENTITY,
            service_descriptions::IDENTITY,
            ServiceKind::Security,
        )
        .with_tags(&[ServiceTag::Core]),
        ServiceStatus::Active,
        Some(service_notes::READY.to_string()),
    );
    let security_manager = Arc::new(wrap_boot(
        SecurityManager::new(&cfg.security, audit_sink),
        BootErrorCode::SecurityInit,
        boot_errors::SECURITY_MANAGER_INIT_FAILED,
    )?);
    services
        .attach_security(Arc::clone(&security_manager))
        .map_err(|err| {
            BootError::new(
                BootErrorCode::SecurityAttach,
                boot_errors::SECURITY_MANAGER_ATTACH_FAILED,
                anyhow!(err),
            )
        })?;
    let session_service = Arc::new(SessionService::new(Arc::clone(&security_manager)));
    services
        .attach_session(Arc::clone(&session_service))
        .map_err(|err| {
            BootError::new(
                BootErrorCode::SessionAttach,
                boot_errors::SESSION_SERVICE_ATTACH_FAILED,
                anyhow!(err),
            )
        })?;

    // Process pending password from setup script (if any)
    if let Some(pending_password) = identity_service.take_pending_password() {
        let configured_user = &cfg.server.ssh.user;
        info!(user = %configured_user, "processing pending password from setup script");

        match security_manager.hash_password(pending_password.as_bytes()) {
            Ok(hash) => {
                if let Err(err) = identity_service.set_user_password(
                    configured_user,
                    &hash,
                    crate::security::auth::Role::Admin,
                ) {
                    warn!(error = %err, "failed to set pending password");
                } else {
                    info!(user = %configured_user, "password configured successfully");
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to hash pending password");
            }
        }
    }

    let default_service_scopes = wrap_boot(
        cfg.modules
            .runtime
            .default_service_scopes
            .iter()
            .map(|scope| ServiceScope::new(scope).map_err(|err| anyhow!(err)))
            .collect::<Result<Vec<_>, _>>(),
        BootErrorCode::ModuleAttach,
        boot_errors::MODULE_SERVICE_SCOPE_INVALID,
    )?;
    let service_overrides = wrap_boot(
        ModuleServiceOverrides::from_config(&cfg.modules.services, &cfg.modules.service_profiles),
        BootErrorCode::ModuleAttach,
        boot_errors::MODULE_SERVICE_ATTACH_FAILED,
    )?;
    let client_settings = wrap_boot(
        ModuleClientSettings::from_config(&cfg.modules.runtime.clients),
        BootErrorCode::ModuleAttach,
        boot_errors::MODULE_SERVICE_ATTACH_FAILED,
    )?;
    let health_client = wrap_boot(
        ModuleHealthHttpClient::new(&client_settings),
        BootErrorCode::ModuleAttach,
        boot_errors::MODULE_SERVICE_ATTACH_FAILED,
    )?;
    let control_plane_url = if cfg.server.enable_http {
        let scheme = if cfg.server.http.tls.enabled {
            "https"
        } else {
            "http"
        };
        Some(format!(
            "{scheme}://{}:{}",
            cfg.server.http.host, cfg.server.http.port
        ))
    } else {
        None
    };
    let module_service = ModuleService::new(ModuleServiceInit {
        registry: Arc::clone(&module_registry),
        storage: Arc::clone(&module_storage),
        verifier: Arc::clone(&module_verifier),
        runtime: Arc::clone(&module_runtime),
        service_registry: Arc::clone(&registry),
        port_allocator: Arc::clone(&port_allocator),
        security: Arc::clone(&security_manager),
        dev_sources,
        overrides: service_overrides,
        client_settings,
        health_client,
        default_service_scopes,
        env_passthrough_prefixes: cfg.modules.runtime.env_passthrough_prefixes.clone(),
        control_plane_url,
        service_snapshot_path: Some(runtime_state_dir.join("services.json")),
        diagnostics: services.diagnostics(),
        services: Arc::downgrade(&services),
    });
    services
        .attach_module_service(Arc::clone(&module_service))
        .map_err(|err| {
            BootError::new(
                BootErrorCode::ModuleAttach,
                boot_errors::MODULE_SERVICE_ATTACH_FAILED,
                anyhow!(err),
            )
        })?;
    let token_exchange_service = Arc::new(TokenExchangeService::new(
        Arc::clone(&module_service),
        Arc::clone(&diagnostics),
    ));
    services
        .attach_token_exchange(Arc::clone(&token_exchange_service))
        .map_err(|err| {
            BootError::new(
                BootErrorCode::ModuleAttach,
                boot_errors::TOKEN_EXCHANGE_ATTACH_FAILED,
                anyhow!(err),
            )
        })?;
    module_service.spawn_health_monitor();
    registry.set_status(
        "module-runtime",
        ServiceStatus::Active,
        Some(service_notes::MODULE_READY.to_string()),
    );
    // Attach security/diagnostics to early db runtime supervisor and register it.
    if let Some(db_runtime) = early_db_runtime {
        db_runtime.attach_security(Arc::clone(&security_manager));
        db_runtime.attach_diagnostics(Arc::clone(&diagnostics));
        // Attach to OnceCell so CLI can query status/logs
        let _ = services.attach_db_runtime(Arc::clone(&db_runtime));
        // Register for ManagedService start/stop
        services.register_runtime_service(db_runtime);
        registry.set_status(
            "db-runtime",
            ServiceStatus::Active,
            Some(service_notes::DB_RUNTIME_READY.to_string()),
        );
    }

    // Create backup service if backup is enabled and db runtime is embedded
    let backup_service =
        if cfg.db.backup.enabled && cfg.db.runtime.mode == crate::config::DbRuntimeMode::Embedded {
            // Get db_runtime from services (already attached above)
            match services.db_runtime() {
                Some(db_runtime) => Some(Arc::new(crate::services::backup::BackupService::new(
                    cfg.db.backup.clone(),
                    cfg.db.runtime.embedded.engine,
                    Arc::clone(&db_shell_service),
                    db_runtime,
                    services.audit_store(),
                    runtime_dir.clone(),
                ))),
                None => {
                    warn!("backup service not created: db_runtime not available");
                    None
                }
            }
        } else {
            None
        };
    if let Some(backup_service) = backup_service.as_ref() {
        let _ = services.attach_backup_service(Arc::clone(backup_service));
    }

    wrap_boot(
        install_default_jobs(
            &scheduler_service,
            SchedulerJobContext {
                registry: Arc::clone(&registry),
                db_shell: Arc::clone(&db_shell_service),
                diagnostics: Arc::clone(&diagnostics),
                module_service: Arc::clone(&module_service),
                services: Arc::clone(&services),
                runtime_dir: runtime_dir.clone(),
                token_exchange: Arc::clone(&token_exchange_service),
                backup_service,
            },
        ),
        BootErrorCode::SchedulerJobs,
        boot_errors::SCHEDULER_JOBS_INSTALL_FAILED,
    )?;

    let http_server = Arc::new(wrap_boot(
        HttpServer::new(&cfg, Arc::clone(&registry), Arc::downgrade(&services)),
        BootErrorCode::HttpServerInit,
        boot_errors::HTTP_SERVER_INIT_FAILED,
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
        let registry_for_token_start = Arc::clone(&registry);
        let registry_for_token_stop = Arc::clone(&registry);
        services.register_dynamic_service(
            "token-exchange",
            move || {
                let registry = Arc::clone(&registry_for_token_start);
                Box::pin(async move {
                    registry.set_status(
                        "token-exchange",
                        ServiceStatus::Active,
                        Some(service_notes::TOKEN_EXCHANGE_READY.to_string()),
                    );
                    Ok(true)
                })
            },
            move |_force| {
                let registry = Arc::clone(&registry_for_token_stop);
                Box::pin(async move {
                    registry.set_status(
                        "token-exchange",
                        ServiceStatus::Standby,
                        Some(service_notes::INITIALIZATION.to_string()),
                    );
                    Ok(true)
                })
            },
        );
    }
    {
        let registry_for_lifecycle_start = Arc::clone(&registry);
        let registry_for_lifecycle_stop = Arc::clone(&registry);
        let module_service_for_lifecycle = Arc::clone(&module_service);
        services.register_dynamic_service(
            "module-lifecycle",
            move || {
                let registry = Arc::clone(&registry_for_lifecycle_start);
                let module_service = Arc::clone(&module_service_for_lifecycle);
                Box::pin(async move {
                    module_service.ensure_all_running().await;
                    registry.set_status(
                        "module-lifecycle",
                        ServiceStatus::Active,
                        Some(service_notes::MODULE_LIFECYCLE_READY.to_string()),
                    );
                    Ok(true)
                })
            },
            move |_force| {
                let registry = Arc::clone(&registry_for_lifecycle_stop);
                Box::pin(async move {
                    registry.set_status(
                        "module-lifecycle",
                        ServiceStatus::Standby,
                        Some(service_notes::INITIALIZATION.to_string()),
                    );
                    Ok(true)
                })
            },
        );
    }
    {
        let registry_for_jobs_control_start = Arc::clone(&registry);
        let registry_for_jobs_control_stop = Arc::clone(&registry);
        let scheduler_for_jobs_control = Arc::clone(&scheduler_service);
        services.register_dynamic_service(
            "jobs-control",
            move || {
                let registry = Arc::clone(&registry_for_jobs_control_start);
                let scheduler = Arc::clone(&scheduler_for_jobs_control);
                Box::pin(async move {
                    if scheduler.is_running() {
                        registry.set_status(
                            "jobs-control",
                            ServiceStatus::Active,
                            Some(service_notes::JOBS_CONTROL_READY.to_string()),
                        );
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })
            },
            move |_force| {
                let registry = Arc::clone(&registry_for_jobs_control_stop);
                Box::pin(async move {
                    registry.set_status(
                        "jobs-control",
                        ServiceStatus::Standby,
                        Some(service_notes::INITIALIZATION.to_string()),
                    );
                    Ok(true)
                })
            },
        );
    }
    {
        let scheduler_for_start = Arc::clone(&scheduler_service);
        let scheduler_for_stop = Arc::clone(&scheduler_service);
        let registry_for_jobs = Arc::clone(&registry);
        let diagnostics_for_jobs = Arc::clone(&diagnostics);
        let db_shell_for_jobs = Arc::clone(&db_shell_service);
        let module_service_for_jobs = Arc::clone(&module_service);
        let services_for_jobs = Arc::clone(&services);
        let runtime_dir_for_jobs = runtime_dir.clone();
        let token_exchange_for_jobs = Arc::clone(&token_exchange_service);
        services.register_dynamic_service(
            "scheduler",
            move || {
                let scheduler = Arc::clone(&scheduler_for_start);
                let registry = Arc::clone(&registry_for_jobs);
                let db_shell = Arc::clone(&db_shell_for_jobs);
                let diagnostics = Arc::clone(&diagnostics_for_jobs);
                let module_service = Arc::clone(&module_service_for_jobs);
                let services_handle = Arc::clone(&services_for_jobs);
                let runtime_dir = runtime_dir_for_jobs.clone();
                let token_exchange = Arc::clone(&token_exchange_for_jobs);
                Box::pin(async move {
                    if scheduler.start() {
                        install_default_jobs(
                            &scheduler,
                            SchedulerJobContext {
                                registry,
                                db_shell,
                                diagnostics,
                                module_service,
                                services: services_handle,
                                runtime_dir,
                                token_exchange,
                                backup_service: None, // Backup service not available on restart
                            },
                        )
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
                            Some(service_notes::DB_SHELL_ENABLED.to_string()),
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
                            Some(service_notes::DB_SHELL_DISABLED.to_string()),
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
        service_notes::CLI_READY_FOR_SESSIONS,
        ServiceStatus::Stopped,
        service_notes::CLI_DISABLED,
    );

    crate::infra::telemetry::mark_ready();

    tracing::info!(
        app = %cfg.app.name,
        version = %cfg.app.version,
        "{}",
        boot_logs::BOOT_COMPLETE
    );
    Ok(BootContext {
        config: Arc::new(cfg),
        services,
        http_server,
        logging: logging_handle,
    })
}

fn control_plane_base_url(http_cfg: &crate::config::HttpConfig) -> String {
    let scheme = if http_cfg.tls.enabled {
        "https"
    } else {
        "http"
    };
    let host = match http_cfg.host.as_str() {
        "0.0.0.0" => "127.0.0.1",
        "::" => "127.0.0.1",
        other => other,
    };
    format!("{}://{}:{}", scheme, host, http_cfg.port)
}
