use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use axum::Router;
use tokio::net::{lookup_host, TcpListener};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::config::AppConfig;
use crate::infra::telemetry;
use crate::security::auth::ControlPlaneAuthorizer;
use crate::services::{AppServices, ManagedService, ServiceRegistry, ServiceStatus};

use super::routes::build_router;
use super::state::{HttpInfo, HttpState};
use super::tls::{HttpTlsProvider, HttpTlsRuntime, TlsReloadEvent, TlsReloadReason};

/// Identifier used in the service registry for the HTTP server.
pub const HTTP_SERVICE_ID: &str = "http-server";

#[derive(Clone)]
struct HttpServerConfig {
    host: String,
    port: u16,
    app_name: String,
    app_version: String,
}

struct ServerHandle {
    join: JoinHandle<()>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl ServerHandle {
    async fn shutdown(mut self, force: bool) -> Result<()> {
        let mut join = self.join;
        if force {
            join.abort();
            return match join.await {
                Ok(_) => Ok(()),
                Err(err) if err.is_cancelled() => Ok(()),
                Err(err) => Err(anyhow!("HTTP server join error: {err}")),
            };
        }

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        let timeout = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(timeout);
        tokio::select! {
            res = &mut join => {
                match res {
                    Ok(_) => Ok(()),
                    Err(err) if err.is_cancelled() => Ok(()),
                    Err(err) => Err(anyhow!("HTTP server join error: {err}")),
                }
            }
            _ = &mut timeout => {
                join.abort();
                match join.await {
                    Ok(_) => Err(anyhow!("HTTP server shutdown timed out; task aborted")),
                    Err(err) if err.is_cancelled() => Err(anyhow!(
                        "HTTP server shutdown timed out; task aborted"
                    )),
                    Err(err) => Err(anyhow!("HTTP server join error after abort: {err}")),
                }
            }
        }
    }
}

pub struct HttpServer {
    config: HttpServerConfig,
    registry: Arc<ServiceRegistry>,
    services: Weak<AppServices>,
    auth: Arc<ControlPlaneAuthorizer>,
    handle: Mutex<Option<ServerHandle>>,
    tls_provider: RwLock<Option<Arc<HttpTlsProvider>>>,
}

pub struct HttpServerControl {
    server: Arc<HttpServer>,
}

impl HttpServerControl {
    pub fn new(server: Arc<HttpServer>) -> Self {
        Self { server }
    }
}

impl HttpServer {
    pub fn new(
        cfg: &AppConfig,
        registry: Arc<ServiceRegistry>,
        services: Weak<AppServices>,
    ) -> Result<Self> {
        let config = HttpServerConfig {
            host: cfg.server.http.host.clone(),
            port: cfg.server.http.port,
            app_name: cfg.app.name.clone(),
            app_version: cfg.app.version.clone(),
        };
        let identity = services.upgrade().and_then(|svc| svc.identity());
        let auth = if let Some(identity) = identity {
            info!("HTTP control-plane authentication via identity broker");
            Arc::new(ControlPlaneAuthorizer::with_identity(identity))
        } else {
            let control_tokens = cfg
                .security
                .http
                .resolve_control_tokens()
                .map_err(|err| anyhow!(err))?;
            let entries = control_tokens
                .into_iter()
                .map(|token| (token.role, token.secret))
                .collect::<Vec<_>>();
            if entries.is_empty() {
                return Err(anyhow!(
                    "no control-plane authentication configured (identity service missing, security.http.control_tokens empty)"
                ));
            }
            info!("HTTP control-plane authentication via static token fallback");
            Arc::new(ControlPlaneAuthorizer::with_tokens(entries))
        };
        let runtime = HttpTlsRuntime::from(&cfg.server.http.tls);
        let tls_provider = if runtime.enabled {
            let provider = Arc::new(HttpTlsProvider::new(runtime.clone())?);
            provider.init_watchers()?;
            provider.spawn_auto_reload();
            Some(provider)
        } else {
            None
        };

        let server = Self {
            config,
            registry,
            services,
            auth,
            handle: Mutex::new(None),
            tls_provider: RwLock::new(tls_provider.clone()),
        };

        if let Some(provider) = tls_provider.as_ref() {
            server.attach_tls_hooks(provider);
            telemetry::set_counter("http.tls.enabled", 1);
        } else {
            telemetry::set_counter("http.tls.enabled", 0);
        }

        Ok(server)
    }

    pub async fn reload_tls(&self, cfg: &crate::config::HttpTlsConfig) -> Result<()> {
        let new_runtime = HttpTlsRuntime::from(cfg);
        new_runtime.validate()?;
        let mut maybe_update: Option<(Arc<HttpTlsProvider>, HttpTlsRuntime)> = None;
        {
            let mut guard = self
                .tls_provider
                .write()
                .map_err(|_| anyhow!("tls provider lock poisoned"))?;
            if new_runtime.enabled {
                if let Some(provider) = guard.as_ref() {
                    maybe_update = Some((Arc::clone(provider), new_runtime.clone()));
                } else {
                    let provider = Arc::new(HttpTlsProvider::new(new_runtime.clone())?);
                    provider.init_watchers()?;
                    provider.spawn_auto_reload();
                    *guard = Some(Arc::clone(&provider));
                    self.attach_tls_hooks(&provider);
                    telemetry::set_counter("http.tls.enabled", 1);
                    info!("HTTP TLS enabled and certificates loaded");
                }
            } else {
                *guard = None;
                telemetry::set_counter("http.tls.enabled", 0);
                info!("HTTP TLS disabled");
            }
        }

        if let Some((provider, runtime)) = maybe_update {
            provider
                .update_runtime(runtime, TlsReloadReason::ConfigReload)
                .await?;
            info!("HTTP TLS configuration reloaded");
        }
        Ok(())
    }

    fn attach_tls_hooks(&self, provider: &Arc<HttpTlsProvider>) {
        let services = self.services.clone();
        let endpoint = Arc::new(format!("{}:{}", self.config.host, self.config.port));
        provider.register_hook(Arc::new(move |event: &TlsReloadEvent| {
            telemetry::record_counter("http.tls.reloads_total", 1);
            if let Ok(epoch) = event.timestamp.duration_since(SystemTime::UNIX_EPOCH) {
                telemetry::set_counter("http.tls.reload_last_epoch", epoch.as_secs());
            }

            let endpoint_ref = endpoint.as_str();
            info!(
                reason = tls_reload_reason_label(&event.reason),
                cert = %event.cert_path.display(),
                key = %event.key_path.display(),
                "HTTP TLS certificates reloaded"
            );

            if let Some(services) = services.upgrade() {
                let metadata = AuditMetadata::default()
                    .insert("reason", tls_reload_reason_label(&event.reason))
                    .insert("endpoint", endpoint_ref)
                    .insert("cert_path", event.cert_path.display().to_string())
                    .insert("key_path", event.key_path.display().to_string());
                match AuditEvent::builder()
                    .actor(AuditActor::System)
                    .action("http.tls.reload")
                    .target(format!("http-tls://{endpoint_ref}"))
                    .outcome(AuditOutcome::Success)
                    .metadata(metadata)
                    .build()
                {
                    Ok(event) => {
                        if let Err(err) = services.record_audit(event) {
                            tracing::warn!(error = %err, "failed to persist audit event for TLS reload");
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to build audit event for TLS reload");
                    }
                }
            }
        }));
    }

    pub fn is_running(&self) -> bool {
        self.handle
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    pub async fn start(self: &Arc<Self>) -> Result<bool> {
        if self.is_running() {
            debug!("HTTP server already running");
            return Ok(false);
        }
        self.registry.set_status(
            HTTP_SERVICE_ID,
            ServiceStatus::Starting,
            Some("initializing".to_string()),
        );

        let mut resolved = lookup_host((self.config.host.as_str(), self.config.port))
            .await
            .with_context(|| {
                format!(
                    "failed to resolve HTTP address: {}:{}",
                    self.config.host, self.config.port
                )
            })?;
        let addr = resolved
            .next()
            .ok_or_else(|| anyhow!("no address resolved for HTTP server"))?;
        let listener = TcpListener::bind(addr)
            .await
            .with_context(|| format!("failed to bind HTTP server: {addr}"))?;
        let actual_addr = listener.local_addr().unwrap_or(addr);
        let services = self
            .services
            .upgrade()
            .ok_or_else(|| anyhow!("app services reference dropped"))?;
        let state = HttpState {
            registry: Arc::clone(&self.registry),
            services: Arc::clone(&services),
            auth: Arc::clone(&self.auth),
            info: HttpInfo {
                app_name: self.config.app_name.clone(),
                app_version: self.config.app_version.clone(),
                host: self.config.host.clone(),
                port: self.config.port,
            },
        };
        let router = Arc::new(build_router(state));
        let tls_provider = self
            .tls_provider
            .read()
            .map_err(|_| anyhow!("tls provider lock poisoned"))?
            .clone();
        let (tx, rx) = oneshot::channel();
        let server = Arc::clone(self);
        let registry = Arc::clone(&self.registry);

        let join = tokio::spawn(async move {
            registry.set_status(
                HTTP_SERVICE_ID,
                ServiceStatus::Active,
                Some(format!("listening on {actual_addr}")),
            );

            let res = if let Some(provider) = tls_provider {
                server
                    .serve_tls(listener, Arc::clone(&router), provider, rx)
                    .await
            } else {
                axum::serve(listener, (*router).clone())
                    .with_graceful_shutdown(async move {
                        let _ = rx.await;
                    })
                    .await
                    .map_err(|err| anyhow!(err))
            };
            server.finish(res).await;
        });

        let mut guard = self
            .handle
            .lock()
            .map_err(|_| anyhow!("http server handle poisoned"))?;
        *guard = Some(ServerHandle {
            join,
            shutdown_tx: Some(tx),
        });
        info!("HTTP server task spawned");
        Ok(true)
    }

    pub async fn stop(self: &Arc<Self>, force: bool) -> Result<bool> {
        let handle = {
            let mut guard = self
                .handle
                .lock()
                .map_err(|_| anyhow!("http server handle poisoned"))?;
            guard.take()
        };
        let Some(handle) = handle else {
            debug!("HTTP server stop requested but server not running");
            return Ok(false);
        };

        let message = if force {
            "shutting down (force)".to_string()
        } else {
            "shutting down".to_string()
        };
        self.registry
            .set_status(HTTP_SERVICE_ID, ServiceStatus::Degraded, Some(message));
        match handle.shutdown(force).await {
            Ok(_) => {
                self.registry.set_status(
                    HTTP_SERVICE_ID,
                    ServiceStatus::Stopped,
                    Some(if force {
                        "stopped (force)".to_string()
                    } else {
                        "stopped".to_string()
                    }),
                );
                info!("HTTP server stopped");
                Ok(true)
            }
            Err(err) => {
                self.registry.set_status(
                    HTTP_SERVICE_ID,
                    ServiceStatus::Failed,
                    Some(format!("error while stopping: {err}")),
                );
                Err(err)
            }
        }
    }

    async fn finish(&self, result: Result<(), anyhow::Error>) {
        if let Err(err) = result {
            warn!(error = %err, "HTTP server terminated with error");
            self.registry.set_status(
                HTTP_SERVICE_ID,
                ServiceStatus::Failed,
                Some(format!("error: {err}")),
            );
        }
        if let Ok(mut guard) = self.handle.lock() {
            if guard.is_some() {
                *guard = None;
            }
        }
    }

    async fn serve_tls(
        &self,
        listener: TcpListener,
        router: Arc<Router>,
        provider: Arc<HttpTlsProvider>,
        mut shutdown: oneshot::Receiver<()>,
    ) -> Result<(), anyhow::Error> {
        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    break;
                }
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((stream, addr)) => {
                            let service = router.clone();
                            let provider = Arc::clone(&provider);
                            tokio::spawn(async move {
                                if let Err(err) = provider.serve_connection(stream, service).await {
                                    tracing::error!(address = %addr, error = %err, "TLS connection failed");
                                }
                            });
                        }
                        Err(err) => {
                            tracing::error!(error = %err, "error accepting TLS connection");
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl ManagedService for HttpServerControl {
    fn id(&self) -> &'static str {
        HTTP_SERVICE_ID
    }

    async fn start(self: Arc<Self>) -> anyhow::Result<bool> {
        HttpServer::start(&self.server).await
    }

    async fn stop(self: Arc<Self>, force: bool) -> anyhow::Result<bool> {
        HttpServer::stop(&self.server, force).await
    }
}

fn tls_reload_reason_label(reason: &TlsReloadReason) -> &'static str {
    match reason {
        TlsReloadReason::Filesystem => "filesystem",
        TlsReloadReason::Interval => "interval",
        TlsReloadReason::ConfigReload => "config_reload",
    }
}
