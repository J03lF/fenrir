use std::sync::Arc;

use crate::config::{self, AppConfig};
use crate::domain::db::DbEngine;
use crate::infra::{db, logging, ssh, telemetry};
use crate::services::{
    db_shell::DbShellService, AppServices, SchedulerService, ServiceDescriptor, ServiceKind,
    ServiceRegistry, ServiceStatus,
};
use anyhow::{anyhow, Result};

pub struct BootContext {
    pub config: Arc<AppConfig>,
    pub services: Arc<AppServices>,
}

pub fn boot() -> Result<BootContext> {
    let cfg = config::load()?;
    logging::init_tracing(&cfg)?;
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
        ),
        ServiceStatus::Active,
        Some("bereit".to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "ssh-server",
            "SSH Transport",
            "Secure Shell Zugang und interaktive Sitzungen",
            ServiceKind::Transport,
        ),
        ServiceStatus::Starting,
        Some("Initialisierung".to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "cli-shell",
            "Lokale CLI",
            "Interaktive CLI-Shell (lokal)",
            ServiceKind::Cli,
        ),
        ServiceStatus::Standby,
        Some("Wartend auf Aufruf".to_string()),
    );
    registry.register(
        ServiceDescriptor::new(
            "scheduler",
            "Background Scheduler",
            "Verwaltet periodische Jobs und Tasks",
            ServiceKind::BackgroundJob,
        ),
        ServiceStatus::Starting,
        Some("Initialisierung".to_string()),
    );

    let db_shell_service = Arc::new(DbShellService::new(default_engine, adapters)?);
    let scheduler_service = Arc::new(SchedulerService::new(Arc::clone(&registry)));
    scheduler_service.start();

    let services = Arc::new(AppServices::new(
        Arc::clone(&db_shell_service),
        Arc::clone(&scheduler_service),
        Arc::clone(&registry),
    ));

    tracing::info!(app = %cfg.app.name, version = %cfg.app.version, "boot complete");
    Ok(BootContext {
        config: Arc::new(cfg),
        services,
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
        if let Err(e) = ssh::start(&cfg, &services).await {
            tracing::error!(error=%e, "ssh server exited with error");
            services.registry().set_status(
                "ssh-server",
                ServiceStatus::Failed,
                Some(format!("Fehler: {e}")),
            );
        }
    });
    Ok(())
}
