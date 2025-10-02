use std::sync::Arc;

use crate::config::{self, AppConfig};
use crate::domain::db::DbEngine;
use crate::infra::http::{HttpServer, HttpServerControl, HTTP_SERVICE_ID};
use crate::infra::storage::memory::{InMemoryTicketRepository, InMemoryUserRepository};
use crate::infra::{db, logging, ssh, telemetry};
use crate::services::db_shell::DbShellControl;
use crate::services::scheduler::{install_default_jobs, SchedulerControl};
use crate::services::{
    AppServices, DbShellService, SchedulerService, ServiceDescriptor, ServiceKind, ServiceRegistry,
    ServiceStatus, ServiceTag, TicketService, UserService,
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
        .with_tags(&[ServiceTag::Platform])
        .critical(),
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
        .with_tags(&[ServiceTag::Platform])
        .critical(),
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
        .with_tags(&[ServiceTag::Platform])
        .critical(),
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
        .with_tags(&[ServiceTag::Auxiliary])
        .critical(),
        ServiceStatus::Standby,
        Some("Wartend auf Aufruf".to_string()),
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

    let services = Arc::new(AppServices::new(
        Arc::clone(&db_shell_service),
        Arc::clone(&scheduler_service),
        Arc::clone(&ticket_service),
        Arc::clone(&user_service),
        Arc::clone(&registry),
    ));
    services.set_logging_handle(logging_handle.clone());

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
    let http_control = Arc::new(HttpServerControl::new(Arc::clone(&http_server)));
    services.register_runtime_service(http_control);
    let db_shell_control = Arc::new(DbShellControl::new(
        Arc::clone(&db_shell_service),
        Arc::clone(&registry),
    ));
    services.register_runtime_service(db_shell_control);
    let scheduler_control = Arc::new(SchedulerControl::new(
        Arc::clone(&scheduler_service),
        Arc::clone(&registry),
        Arc::clone(&db_shell_service),
    ));
    services.register_runtime_service(scheduler_control);

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
