use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Context, Result};
use arc_swap::ArcSwap;
use axum::Router;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Request as HyperRequest, Response as HyperResponse};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as HyperBuilder;
use hyper_util::service::TowerToHyperService;
use notify::{Config as NotifyConfig, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rustls::{self, ServerConfig as RustlsServerConfig};
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use std::convert::Infallible;
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use tower::service_fn;
use tower::util::ServiceExt;

use crate::config::HttpTlsConfig;
use crate::utils::messages;

type TlsReloadHook = dyn Fn(&TlsReloadEvent) + Send + Sync + 'static;

#[derive(Clone, Debug)]
pub(super) enum TlsReloadReason {
    Filesystem,
    Interval,
    ConfigReload,
}

#[derive(Clone, Debug)]
pub(super) struct TlsReloadEvent {
    pub reason: TlsReloadReason,
    pub timestamp: SystemTime,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Clone)]
pub(super) struct HttpTlsRuntime {
    pub(super) enabled: bool,
    pub(super) cert_path: PathBuf,
    pub(super) key_path: PathBuf,
    pub(super) cipher_suites: Vec<String>,
    pub(super) reload_interval: Option<Duration>,
}

impl HttpTlsRuntime {
    pub(super) fn from(cfg: &HttpTlsConfig) -> Self {
        Self {
            enabled: cfg.enabled,
            cert_path: cfg
                .cert_path
                .as_ref()
                .map(|p| PathBuf::from(p.trim()))
                .unwrap_or_default(),
            key_path: cfg
                .key_path
                .as_ref()
                .map(|p| PathBuf::from(p.trim()))
                .unwrap_or_default(),
            cipher_suites: cfg.cipher_suites.clone(),
            reload_interval: cfg.reload_interval_seconds.map(Duration::from_secs),
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.enabled {
            if self.cert_path.as_os_str().is_empty() {
                return Err(anyhow!(messages::infra::http::tls::CERT_PATH_REQUIRED));
            }
            if self.key_path.as_os_str().is_empty() {
                return Err(anyhow!(messages::infra::http::tls::KEY_PATH_REQUIRED));
            }
        }
        Ok(())
    }
}

pub(super) struct HttpTlsProvider {
    runtime: RwLock<HttpTlsRuntime>,
    config: ArcSwap<RustlsServerConfig>,
    next_reload: Mutex<Option<Instant>>,
    watcher: Mutex<Option<TlsFileWatcher>>,
    hooks: RwLock<Vec<Arc<TlsReloadHook>>>,
}

struct TlsFileWatcher {
    shutdown: mpsc::Sender<()>,
    thread: thread::JoinHandle<()>,
}

fn tls_event_requires_reload(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

impl HttpTlsProvider {
    pub(super) fn new(runtime: HttpTlsRuntime) -> Result<Self> {
        runtime.validate()?;
        let config = load_server_config_sync(&runtime)?;
        let provider = Self {
            next_reload: Mutex::new(
                runtime
                    .reload_interval
                    .map(|interval| Instant::now() + interval),
            ),
            runtime: RwLock::new(runtime),
            config: ArcSwap::from_pointee(config),
            watcher: Mutex::new(None),
            hooks: RwLock::new(Vec::new()),
        };
        Ok(provider)
    }

    pub(super) fn init_watchers(self: &Arc<Self>) -> Result<()> {
        let runtime = self
            .runtime
            .read()
            .map_err(|_| anyhow!(messages::infra::http::tls::RUNTIME_LOCK_POISONED))?
            .clone();
        if runtime.enabled {
            self.start_watcher(runtime)?;
        }
        Ok(())
    }

    pub(super) fn restart_watcher(self: &Arc<Self>, runtime: HttpTlsRuntime) -> Result<()> {
        self.stop_watcher();
        if runtime.enabled {
            self.start_watcher(runtime)?;
        }
        Ok(())
    }

    fn start_watcher(self: &Arc<Self>, runtime: HttpTlsRuntime) -> Result<()> {
        let cert_path = runtime.cert_path.clone();
        let key_path = runtime.key_path.clone();
        if !cert_path.exists() {
            return Err(anyhow!(
                "{}",
                messages::infra::http::tls::cert_not_found(cert_path.display())
            ));
        }
        if !key_path.exists() {
            return Err(anyhow!(
                "{}",
                messages::infra::http::tls::key_not_found(key_path.display())
            ));
        }
        let weak = Arc::downgrade(self);
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("http-tls-watch".to_string())
            .spawn(move || {
                let (event_tx, event_rx) = mpsc::channel();
                let mut watcher = match RecommendedWatcher::new(
                    move |res| {
                        let _ = event_tx.send(res);
                    },
                    NotifyConfig::default(),
                ) {
                    Ok(watcher) => watcher,
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            "{}",
                            messages::infra::http::tls::FILE_WATCHER_CREATE_FAILED
                        );
                        return;
                    }
                };

                if let Err(err) = watcher.watch(&cert_path, RecursiveMode::NonRecursive) {
                    tracing::error!(
                        path = %cert_path.display(),
                        error = %err,
                        "{}",
                        messages::infra::http::tls::CERT_WATCH_FAILED
                    );
                    return;
                }
                if key_path != cert_path {
                    if let Err(err) = watcher.watch(&key_path, RecursiveMode::NonRecursive) {
                        tracing::error!(
                            path = %key_path.display(),
                            error = %err,
                            "{}",
                            messages::infra::http::tls::KEY_WATCH_FAILED
                        );
                        return;
                    }
                }

                loop {
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }
                    match event_rx.recv_timeout(Duration::from_secs(1)) {
                        Ok(Ok(event)) => {
                            if tls_event_requires_reload(&event.kind) {
                                if let Some(provider) = weak.upgrade() {
                                    if let Err(err) =
                                        provider.refresh_sync(TlsReloadReason::Filesystem)
                                    {
                                        tracing::warn!(
                                            error = %err,
                                            "{}",
                                            messages::infra::http::tls::RELOAD_FAILED
                                        );
                                    } else {
                                        tracing::info!(
                                            "{}",
                                            messages::infra::http::tls::RELOAD_SUCCESS_FILESYSTEM
                                        );
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        Ok(Err(err)) => {
                            tracing::warn!(
                                error = %err,
                                "{}",
                                messages::infra::http::tls::WATCH_ERROR
                            );
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;

        let mut guard = self
            .watcher
            .lock()
            .map_err(|_| anyhow!(messages::infra::http::tls::WATCHER_LOCK_POISONED))?;
        *guard = Some(TlsFileWatcher {
            shutdown: shutdown_tx,
            thread: handle,
        });
        Ok(())
    }

    pub(super) fn stop_watcher(&self) {
        if let Ok(mut guard) = self.watcher.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.shutdown.send(());
                let _ = handle.thread.join();
            }
        }
    }

    fn refresh_sync(&self, reason: TlsReloadReason) -> Result<()> {
        let runtime = self
            .runtime
            .read()
            .map_err(|_| anyhow!(messages::infra::http::tls::RUNTIME_LOCK_POISONED))?
            .clone();
        let config = load_server_config_sync(&runtime)?;
        self.config.store(Arc::new(config));
        if let Ok(mut guard) = self.next_reload.lock() {
            *guard = runtime
                .reload_interval
                .map(|interval| Instant::now() + interval);
        }
        self.notify_hooks(runtime, reason);
        Ok(())
    }

    pub(super) fn spawn_auto_reload(self: &Arc<Self>) {
        let interval = {
            let runtime = self
                .runtime
                .read()
                .expect(messages::infra::http::tls::RUNTIME_LOCK_POISONED);
            runtime.reload_interval
        };
        if let Some(interval) = interval {
            let this = Arc::clone(self);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    if let Err(err) = this.reload_now(TlsReloadReason::Interval).await {
                        tracing::warn!(
                            error = %err,
                            "{}",
                            messages::infra::http::tls::RELOAD_FAILED
                        );
                    }
                }
            });
        }
    }

    pub(super) async fn serve_connection(
        &self,
        stream: TcpStream,
        service: Arc<Router>,
    ) -> Result<(), anyhow::Error> {
        self.reload_if_due().await?;
        let config = self.config.load_full();
        let acceptor = TlsAcceptor::from(config);
        let tls_stream = acceptor.accept(stream).await?;
        let io = TokioIo::new(tls_stream);
        let builder = HyperBuilder::new(TokioExecutor::new());
        let router = service.clone();
        let tower_svc = service_fn(move |req: HyperRequest<Incoming>| {
            let router = router.clone();
            async move {
                let (parts, body) = req.into_parts();
                let axum_body = axum::body::Body::from_stream(body.into_data_stream());
                let axum_req = HyperRequest::from_parts(parts, axum_body);
                let router_cloned = (*router).clone();
                let response = match router_cloned.oneshot(axum_req).await {
                    Ok(resp) => resp,
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            "{}",
                            messages::infra::http::tls::REQUEST_FAILED
                        );
                        let body = axum::body::Body::from(
                            messages::infra::http::tls::INTERNAL_SERVER_ERROR_BODY,
                        );
                        let response = HyperResponse::builder()
                            .status(hyper::StatusCode::INTERNAL_SERVER_ERROR)
                            .body(body)
                            .unwrap();
                        return Ok::<_, Infallible>(response);
                    }
                };
                let (parts, body) = response.into_parts();
                let hyper_body = axum::body::Body::from_stream(body.into_data_stream());
                Ok::<_, Infallible>(HyperResponse::from_parts(parts, hyper_body))
            }
        });
        let svc = TowerToHyperService::new(tower_svc);
        builder
            .serve_connection(io, svc)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    pub(super) async fn reload_now(&self, reason: TlsReloadReason) -> Result<()> {
        let runtime = self
            .runtime
            .read()
            .map_err(|_| anyhow!(messages::infra::http::tls::RUNTIME_LOCK_POISONED))?
            .clone();
        let config = load_server_config_async(&runtime).await?;
        self.config.store(Arc::new(config));
        if let Ok(mut guard) = self.next_reload.lock() {
            *guard = runtime
                .reload_interval
                .map(|interval| Instant::now() + interval);
        }
        self.notify_hooks(runtime, reason);
        Ok(())
    }

    async fn reload_if_due(&self) -> Result<()> {
        let reload_due = {
            let mut guard = self
                .next_reload
                .lock()
                .map_err(|_| anyhow!(messages::infra::http::tls::RELOAD_LOCK_POISONED))?;
            if let Some(next) = *guard {
                if Instant::now() >= next {
                    *guard = None;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if reload_due {
            self.reload_now(TlsReloadReason::Interval).await?;
        }
        Ok(())
    }

    pub(super) async fn update_runtime(
        self: &Arc<Self>,
        runtime: HttpTlsRuntime,
        reason: TlsReloadReason,
    ) -> Result<()> {
        runtime.validate()?;
        {
            let mut guard = self
                .runtime
                .write()
                .map_err(|_| anyhow!(messages::infra::http::tls::RUNTIME_LOCK_POISONED))?;
            *guard = runtime.clone();
            if let Some(interval) = runtime.reload_interval {
                if let Ok(mut next) = self.next_reload.lock() {
                    *next = Some(Instant::now() + interval);
                }
            }
        }
        let config = load_server_config_async(&runtime).await?;
        self.config.store(Arc::new(config));
        self.restart_watcher(runtime.clone())?;
        self.notify_hooks(runtime, reason);
        Ok(())
    }

    pub(super) fn register_hook(&self, hook: Arc<TlsReloadHook>) {
        if let Ok(mut guard) = self.hooks.write() {
            guard.push(hook);
        } else {
            tracing::warn!("{}", messages::infra::http::tls::HOOKS_REGISTER_POISONED);
        }
    }

    fn notify_hooks(&self, runtime: HttpTlsRuntime, reason: TlsReloadReason) {
        let event = TlsReloadEvent {
            reason,
            timestamp: SystemTime::now(),
            cert_path: runtime.cert_path,
            key_path: runtime.key_path,
        };
        let hooks = match self.hooks.read() {
            Ok(guard) => guard.clone(),
            Err(_) => {
                tracing::warn!("{}", messages::infra::http::tls::HOOKS_NOTIFY_POISONED);
                return;
            }
        };
        for hook in hooks {
            let snapshot = event.clone();
            if std::panic::catch_unwind(AssertUnwindSafe(|| {
                (hook)(&snapshot);
            }))
            .is_err()
            {
                tracing::warn!("{}", messages::infra::http::tls::HOOK_PANICKED);
            }
        }
    }
}

impl Drop for HttpTlsProvider {
    fn drop(&mut self) {
        self.stop_watcher();
    }
}

fn load_server_config_sync(runtime: &HttpTlsRuntime) -> Result<RustlsServerConfig> {
    let cert_bytes = std::fs::read(&runtime.cert_path).with_context(|| {
        messages::infra::http::tls::cert_read_failed(runtime.cert_path.display())
    })?;
    let key_bytes = std::fs::read(&runtime.key_path)
        .with_context(|| messages::infra::http::tls::key_read_failed(runtime.key_path.display()))?;
    build_server_config(runtime, cert_bytes, key_bytes)
}

async fn load_server_config_async(runtime: &HttpTlsRuntime) -> Result<RustlsServerConfig> {
    let cert_path = runtime.cert_path.clone();
    let key_path = runtime.key_path.clone();
    let cert_bytes = tokio::fs::read(&cert_path)
        .await
        .with_context(|| messages::infra::http::tls::cert_read_failed(cert_path.display()))?;
    let key_bytes = tokio::fs::read(&key_path)
        .await
        .with_context(|| messages::infra::http::tls::key_read_failed(key_path.display()))?;
    build_server_config(runtime, cert_bytes, key_bytes)
}

fn build_server_config(
    runtime: &HttpTlsRuntime,
    cert_bytes: Vec<u8>,
    key_bytes: Vec<u8>,
) -> Result<RustlsServerConfig> {
    let mut cert_reader: &[u8] = &cert_bytes;
    let cert_chain = certs(&mut cert_reader)
        .map_err(|_| anyhow!(messages::infra::http::tls::CERT_PARSE_FAILED))?
        .into_iter()
        .map(rustls::Certificate)
        .collect::<Vec<_>>();
    if cert_chain.is_empty() {
        return Err(anyhow!(messages::infra::http::tls::CERT_CHAIN_EMPTY));
    }

    let mut key_reader: &[u8] = &key_bytes;
    let mut keys = pkcs8_private_keys(&mut key_reader)
        .map_err(|_| anyhow!(messages::infra::http::tls::PKCS8_PARSE_FAILED))?;
    if keys.is_empty() {
        key_reader = &key_bytes;
        keys = rsa_private_keys(&mut key_reader)
            .map_err(|_| anyhow!(messages::infra::http::tls::RSA_PARSE_FAILED))?;
    }
    let key = keys
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!(messages::infra::http::tls::PRIVATE_KEY_MISSING))?;
    let key = rustls::PrivateKey(key);

    let cipher_suites = resolve_cipher_suites(&runtime.cipher_suites)?;

    let mut config = rustls::ServerConfig::builder()
        .with_cipher_suites(cipher_suites.as_slice())
        .with_safe_default_kx_groups()
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

fn resolve_cipher_suites(names: &[String]) -> Result<Vec<rustls::SupportedCipherSuite>> {
    if names.is_empty() {
        return Ok(vec![
            rustls::cipher_suite::TLS13_AES_256_GCM_SHA384,
            rustls::cipher_suite::TLS13_AES_128_GCM_SHA256,
            rustls::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
        ]);
    }

    names
        .iter()
        .map(|name| match name.as_str() {
            "TLS_AES_256_GCM_SHA384" => Ok(rustls::cipher_suite::TLS13_AES_256_GCM_SHA384),
            "TLS_AES_128_GCM_SHA256" => Ok(rustls::cipher_suite::TLS13_AES_128_GCM_SHA256),
            "TLS_CHACHA20_POLY1305_SHA256" => {
                Ok(rustls::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256)
            }
            other => Err(anyhow!(
                "{}",
                messages::infra::http::tls::cipher_suite_unknown(other)
            )),
        })
        .collect()
}
