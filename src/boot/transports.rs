use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use crate::infra::http::HTTP_SERVICE_ID;
use crate::infra::{logging, ssh, telemetry};
use crate::services::ServiceStatus;
use crate::utils::messages::boot::{
    config_reload as config_reload_messages, transports as transport_messages,
};

use super::bootstrap::bootstrap_modules;
use super::context::BootContext;

pub async fn start_transports(ctx: &BootContext) -> Result<()> {
    let module_service = ctx.services.module_service();
    if !ctx.config.modules.bootstrap.is_empty() {
        match module_service.clone() {
            Some(module_service) => {
                bootstrap_modules(Arc::clone(&ctx.config), module_service).await;
            }
            None => {
                warn!("{}", transport_messages::MODULE_SERVICE_NOT_ATTACHED);
            }
        }
    }
    if let Some(service) = module_service {
        service.ensure_all_running().await;
    }

    let cfg = Arc::clone(&ctx.config);
    let services = Arc::clone(&ctx.services);
    tokio::spawn(async move {
        services.registry().set_status(
            "ssh-server",
            ServiceStatus::Starting,
            Some(transport_messages::SSH_STARTING_NOTE.to_string()),
        );
        info!("{}", transport_messages::SSH_TASK_SPAWNED);
        if let Err(e) = ssh::start(&cfg, &services).await {
            tracing::error!(error=%e, "{}", transport_messages::SSH_SERVER_EXITED_WITH_ERROR);
            services.registry().set_status(
                "ssh-server",
                ServiceStatus::Failed,
                Some(transport_messages::ssh_failure_note(&e)),
            );
        }
    });

    if ctx.config.server.enable_http {
        let registry = ctx.services.registry();
        registry.set_status(
            HTTP_SERVICE_ID,
            ServiceStatus::Starting,
            Some(transport_messages::HTTP_STARTING_NOTE.to_string()),
        );
        let http_server = Arc::clone(&ctx.http_server);
        tokio::spawn(async move {
            if let Err(err) = http_server.start().await {
                tracing::error!(
                    error = %err,
                    "{}",
                    transport_messages::HTTP_SERVER_START_FAILED
                );
                registry.set_status(
                    HTTP_SERVICE_ID,
                    ServiceStatus::Failed,
                    Some(transport_messages::http_start_failure_note(&err)),
                );
            }
        });
    }
    spawn_config_reloader(ctx)?;
    info!("{}", transport_messages::TRANSPORT_INITIALISATION_TRIGGERED);
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
                    tracing::warn!(
                        error = %err,
                        "{}",
                        config_reload_messages::SIGNAL_HANDLER_INIT_FAILED
                    );
                    return;
                }
            };

            while hup.recv().await.is_some() {
                match crate::config::load() {
                    Ok(new_cfg) => {
                        if let Err(err) =
                            logging::reload(&logging, &new_cfg.telemetry.tracing.level)
                        {
                            tracing::warn!(
                                error = %err,
                                "{}",
                                config_reload_messages::LOG_LEVEL_UPDATE_FAILED
                            );
                        }
                        telemetry::reload(&new_cfg);
                        if let Err(err) = http_server.reload_tls(&new_cfg.server.http.tls).await {
                            tracing::warn!(
                                error = %err,
                                "{}",
                                config_reload_messages::TLS_RELOAD_FAILED
                            );
                        }
                        tracing::info!("{}", config_reload_messages::CONFIG_RELOADED);
                    }
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "{}",
                            config_reload_messages::CONFIG_RELOAD_FAILED
                        )
                    }
                }
            }
        });
    }

    #[cfg(not(unix))]
    {
        tracing::info!("{}", config_reload_messages::HOT_RELOAD_UNSUPPORTED);
    }

    Ok(())
}
