use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use notify::{Config as NotifyConfig, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::infra::db::connector::start_connector_server;
use crate::infra::http::HTTP_SERVICE_ID;
use crate::infra::{logging, ssh, telemetry};
use crate::security::service::ServiceScope;
use crate::services::{
    DbConnectorService, ModuleService, ServiceDescriptor, ServiceKind, ServiceSecurityMetadata,
    ServiceStatus, ServiceTag, ServiceTenantGuard,
};
use crate::utils::messages::boot::{
    config_reload as config_reload_messages,
    services::{
        descriptions as service_descriptions, names as service_names, notes as service_notes,
    },
    transports as transport_messages,
};

use super::bootstrap::bootstrap_modules;
use super::context::BootContext;

pub async fn start_transports(ctx: &BootContext) -> Result<()> {
    initialize_db_connector(ctx).await?;
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

async fn initialize_db_connector(ctx: &BootContext) -> Result<()> {
    let runtime_dir = env::var("FENRIR_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::temp_dir());
    let Some(module_service) = ctx.services.module_service() else {
        return Ok(());
    };
    let Some(security_manager) = ctx.services.security_manager() else {
        return Ok(());
    };
    let connector_service = Arc::new(DbConnectorService::new(
        Arc::clone(&ctx.services.db_shell),
        security_manager,
        ctx.services.diagnostics(),
    ));
    let endpoint = start_connector_server(&runtime_dir, Arc::clone(&connector_service))
        .await
        .context("db connector startup failed")?;
    module_service.configure_db_connector(Some(endpoint));
    ctx.services.registry().register(
        ServiceDescriptor::new(
            "db-connector",
            service_names::DB_CONNECTOR,
            service_descriptions::DB_CONNECTOR,
            ServiceKind::Infrastructure,
        )
        .with_tags(&[ServiceTag::Platform])
        .with_security(ServiceSecurityMetadata {
            internal_only: true,
            allowed_roles: ServiceSecurityMetadata::default_allowed_roles(),
            required_scopes: vec![
                ServiceScope::new("db:read").expect("db:read scope"),
                ServiceScope::new("db:write").expect("db:write scope"),
            ],
            tenant: ServiceTenantGuard::any(),
        }),
        ServiceStatus::Active,
        Some(service_notes::READY.to_string()),
    );
    Ok(())
}

fn spawn_config_reloader(ctx: &BootContext) -> Result<()> {
    let logging = ctx.logging.clone();
    let http_server = Arc::clone(&ctx.http_server);
    let module_service = ctx.services.module_service();
    let rollout_cfg = ctx.config.modules.runtime.rollout.clone();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let logging = logging.clone();
        let http_server = Arc::clone(&http_server);
        let module_service = module_service.clone();
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
                telemetry::record_counter("config.reload.signal_total", 1);
                apply_runtime_config_reload(
                    &logging,
                    &http_server,
                    module_service.clone(),
                    "signal",
                )
                .await;
            }
        });
    }

    let watch_paths = config_watch_paths();
    if rollout_cfg.watch_config && !watch_paths.is_empty() {
        let logging = logging.clone();
        let http_server = Arc::clone(&http_server);
        let debounce = Duration::from_millis(rollout_cfg.watch_debounce_ms);
        tokio::spawn(async move {
            let (event_tx, mut event_rx) = mpsc::unbounded_channel();
            let mut watcher = match RecommendedWatcher::new(
                move |res| {
                    let _ = event_tx.send(res);
                },
                NotifyConfig::default(),
            ) {
                Ok(watcher) => watcher,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        "{}",
                        config_reload_messages::CONFIG_RELOAD_FAILED
                    );
                    return;
                }
            };

            for path in &watch_paths {
                let mode = if path.is_dir() {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                if let Err(err) = watcher.watch(path, mode) {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "{}",
                        config_reload_messages::CONFIG_RELOAD_FAILED
                    );
                }
            }

            let mut last_reload = Instant::now()
                .checked_sub(debounce)
                .unwrap_or_else(Instant::now);

            while let Some(event) = event_rx.recv().await {
                match event {
                    Ok(event) if config_event_requires_reload(&event.kind) => {
                        if last_reload.elapsed() < debounce {
                            continue;
                        }
                        last_reload = Instant::now();
                        telemetry::record_counter("config.reload.filesystem_total", 1);
                        apply_runtime_config_reload(
                            &logging,
                            &http_server,
                            module_service.clone(),
                            "filesystem",
                        )
                        .await;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "{}",
                            config_reload_messages::CONFIG_RELOAD_FAILED
                        );
                    }
                }
            }
        });
    } else if !rollout_cfg.watch_config {
        tracing::info!(
            "automatic config file watching disabled by modules.runtime.rollout.watch_config"
        );
    }

    #[cfg(not(unix))]
    {
        tracing::info!("{}", config_reload_messages::HOT_RELOAD_UNSUPPORTED);
    }

    Ok(())
}

async fn apply_runtime_config_reload(
    logging_handle: &logging::ReloadHandle,
    http_server: &Arc<crate::infra::http::HttpServer>,
    module_service: Option<Arc<ModuleService>>,
    trigger: &str,
) {
    match crate::config::load() {
        Ok(new_cfg) => {
            if let Err(err) = logging::reload(logging_handle, &new_cfg.telemetry.tracing.level) {
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

            if let Some(service) = module_service {
                match crate::services::module::ModuleServiceOverrides::from_config(
                    &new_cfg.modules.services,
                    &new_cfg.modules.service_profiles,
                ) {
                    Ok(overrides) => match service
                        .reload_service_overrides(
                            overrides,
                            new_cfg.modules.runtime.rollout.restart_on_override_change,
                        )
                        .await
                    {
                        Ok(report) => {
                            tracing::info!(
                                trigger,
                                status = ?report.status,
                                restarted = report.restarted_modules.len(),
                                rolled_back = report.rollback_restarted_modules.len(),
                                "module overrides reloaded automatically"
                            );
                        }
                        Err(err) => {
                            tracing::warn!(
                                trigger,
                                error = %err,
                                "automatic module override reload failed"
                            );
                        }
                    },
                    Err(err) => {
                        tracing::warn!(
                            trigger,
                            error = %err,
                            "automatic module override config invalid"
                        );
                    }
                }
            }

            telemetry::record_counter("config.reload.success_total", 1);
            tracing::info!(trigger, "{}", config_reload_messages::CONFIG_RELOADED);
        }
        Err(err) => {
            telemetry::record_counter("config.reload.failure_total", 1);
            tracing::warn!(
                trigger,
                error = %err,
                "{}",
                config_reload_messages::CONFIG_RELOAD_FAILED
            )
        }
    }
}

fn config_event_requires_reload(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

fn config_watch_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    push_existing_path(&mut paths, Path::new("config"));
    push_existing_path(&mut paths, Path::new("bin/.fenrir-profile"));
    push_existing_path(&mut paths, Path::new("secrets/.env"));
    if let Some(path) = env::var_os("FENRIR_ENV_FILE") {
        push_existing_path(&mut paths, Path::new(&path));
    }
    if let Some(path) = env::var_os("FENRIR_CONFIG_FILE") {
        push_existing_path(&mut paths, Path::new(&path));
    }
    paths
}

fn push_existing_path(paths: &mut Vec<PathBuf>, path: &Path) {
    if path.exists() && !paths.iter().any(|existing| existing == path) {
        paths.push(path.to_path_buf());
    }
}
