use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::audit::InMemoryAuditLog;
use crate::config::{self, AppConfig};
use crate::domain::db::DbEngine;
use crate::infra::http::{HttpServer, HTTP_SERVICE_ID};
use crate::infra::modules::{
    registry::HttpModuleRegistry, Ed25519ModuleVerifier, FilesystemModuleStorage,
    ProcessModuleRuntime,
};
use crate::infra::storage::memory::{InMemoryTicketRepository, InMemoryUserRepository};
use crate::infra::{db, logging, ssh, telemetry};
use crate::services::scheduler::install_default_jobs;
use crate::services::{
    AppServices, DbShellService, ModuleService, SchedulerService, ServiceDescriptor, ServiceKind,
    ServiceRegistry, ServiceStatus, ServiceTag, TicketService, UserService,
};
use anyhow::{anyhow, Result};
use tracing::info;

pub struct BootContext {
    pub config: Arc<AppConfig>,
    pub services: Arc<AppServices>,
    pub http_server: Arc<HttpServer>,
    pub logging: crate::infra::logging::ReloadHandle,
}

pub fn boot() -> Result<BootContext> {
    let cfg = config::load()?;
    let logging_handle = logging::init_tracing(&cfg)?;
    telemetry::init(&cfg)?;

    let adapters = db::manager::build_adapters(&cfg)?;
    let default_engine = cfg
        .db
        .default_engine
        .parse::<DbEngine>()
        .map_err(|err| anyhow!("ungültiger DB-Typ: {err}"))?;
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
            "user-service",
            "User Service",
            "Verwaltet Benutzer und Rollen",
            ServiceKind::Security,
        )
        .with_tags(&[ServiceTag::Platform]),
        ServiceStatus::Starting,
        Some("Initialisierung".to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "ticket-service",
            "Ticket Service",
            "Kern-Use-Cases für das Ticketsystem",
            ServiceKind::Infrastructure,
        )
        .with_tags(&[ServiceTag::Platform]),
        ServiceStatus::Starting,
        Some("Initialisierung".to_string()),
    );
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

    crate::infra::telemetry::register_readiness_probe("services", {
        let registry = Arc::clone(&registry);
        move || {
            registry
                .snapshot()
                .into_iter()
                .all(|svc| !matches!(svc.status, ServiceStatus::Failed))
        }
    })?;

    let db_shell_service = Arc::new(DbShellService::new(default_engine, adapters)?);
    let scheduler_service = Arc::new(SchedulerService::new(Arc::clone(&registry)));
    let user_repository: Arc<dyn crate::domain::user::UserRepository> =
        Arc::new(InMemoryUserRepository::new());
    let user_service = Arc::new(UserService::new(Arc::clone(&user_repository)));
    let ticket_repository: Arc<dyn crate::domain::ticket::TicketRepository> =
        Arc::new(InMemoryTicketRepository::new());
    let ticket_service = Arc::new(TicketService::new(Arc::clone(&ticket_repository)));
    scheduler_service.start();
    info!("scheduler service started");

    let audit_capacity = if cfg.audit.enabled { 1024 } else { 0 };
    let audit_log: Arc<dyn crate::audit::AuditLog> =
        Arc::new(InMemoryAuditLog::new(audit_capacity));

    let module_registry: Arc<dyn crate::domain::module::ModuleRegistryPort> = Arc::new(
        HttpModuleRegistry::new(&cfg.modules.registry)
            .map_err(|err| anyhow!("module registry init failed: {err}"))?,
    );
    let module_storage: Arc<dyn crate::domain::module::ModuleStoragePort> = Arc::new(
        FilesystemModuleStorage::new(&cfg.modules.storage)
            .map_err(|err| anyhow!("module storage init failed: {err}"))?,
    );
    let module_verifier: Arc<dyn crate::domain::module::ModuleVerifierPort> = Arc::new(
        Ed25519ModuleVerifier::from_config(&cfg.modules.trust)
            .map_err(|err| anyhow!("module verifier init failed: {err}"))?,
    );
    let runtime_state_dir = PathBuf::from(&cfg.modules.storage.install_dir).join("runtime");
    let module_runtime: Arc<dyn crate::domain::module::ModuleRuntimePort> = Arc::new(
        ProcessModuleRuntime::new(Arc::clone(&module_storage), runtime_state_dir),
    );
    let module_service = Arc::new(ModuleService::new(
        Arc::clone(&module_registry),
        Arc::clone(&module_storage),
        Arc::clone(&module_verifier),
        Arc::clone(&module_runtime),
    ));

    let services = Arc::new(AppServices::new(
        Arc::clone(&db_shell_service),
        Arc::clone(&scheduler_service),
        Arc::clone(&ticket_service),
        Arc::clone(&user_service),
        Arc::clone(&registry),
        Arc::clone(&audit_log),
    ));
    services.set_logging_handle(logging_handle.clone());
    services
        .attach_module_service(Arc::clone(&module_service))
        .map_err(|err| anyhow!("module service attach failed: {err}"))?;
    registry.set_status(
        "module-runtime",
        ServiceStatus::Active,
        Some("Bereit für Module".to_string()),
    );

    install_default_jobs(
        &scheduler_service,
        Arc::clone(&registry),
        Arc::clone(&db_shell_service),
    )
    .map_err(|err| anyhow!(err))?;

    let http_server = Arc::new(HttpServer::new(
        &cfg,
        Arc::clone(&registry),
        Arc::downgrade(&services),
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
        "user-service",
        ServiceStatus::Active,
        "Service aktiv",
        ServiceStatus::Standby,
        "Service gestoppt",
    );
    register_registry_toggle_service(
        &services,
        &registry,
        "ticket-service",
        ServiceStatus::Active,
        "Service aktiv",
        ServiceStatus::Standby,
        "Service gestoppt",
    );
    register_registry_toggle_service(
        &services,
        &registry,
        "cli-shell",
        ServiceStatus::Standby,
        "Bereit für neue Sessions",
        ServiceStatus::Stopped,
        "CLI deaktiviert",
    );

    registry.set_status(
        "user-service",
        ServiceStatus::Active,
        Some("In-Memory Repository initialisiert".to_string()),
    );
    registry.set_status(
        "ticket-service",
        ServiceStatus::Active,
        Some("In-Memory Repository initialisiert".to_string()),
    );
    info!("user and ticket services initialised");

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
                            logging::reload(&logging, &new_cfg.telemetry.tracing_level)
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
