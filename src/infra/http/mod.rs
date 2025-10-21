use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Context, Result};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{
    sse::{Event, KeepAlive, Sse},
    Html, IntoResponse, Response,
};
use axum::routing::{delete, get, post};
use axum::Json;
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
use serde::{Deserialize, Serialize};
use serde_json;
use std::convert::Infallible;
use time::format_description::well_known::Rfc3339;
use time::Duration as TimeDuration;
use time::OffsetDateTime;
use tokio::net::{lookup_host, TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_stream::{
    wrappers::{errors::BroadcastStreamRecvError, BroadcastStream},
    StreamExt,
};
use tower::service_fn;
use tower::util::ServiceExt;
use tracing::{debug, info, warn};

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::config::AppConfig;
use crate::domain::module::{
    InstalledModule, ModuleError, ModuleId, ModuleInstallResult, ModuleInstallStatus,
    ModuleManifest, ModuleRegistryError, ModuleSearchQuery, ModuleServiceError, ModuleStorageError,
    ModuleVersion,
};
use crate::infra::{logging, telemetry};
use crate::security::auth::{AuthError, ControlPlaneAuthorizer, Role};
use crate::services::scheduler::ScheduledJobSnapshot;
use crate::services::{
    AppServices, ManagedService, ServiceControlError, ServiceRegistry, ServiceSnapshot,
    ServiceStatus,
};

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

#[derive(Clone)]
struct HttpTlsRuntime {
    enabled: bool,
    cert_path: PathBuf,
    key_path: PathBuf,
    cipher_suites: Vec<String>,
    reload_interval: Option<Duration>,
}

impl HttpTlsRuntime {
    fn from(cfg: &crate::config::HttpTlsConfig) -> Self {
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

    fn validate(&self) -> Result<()> {
        if self.enabled {
            if self.cert_path.as_os_str().is_empty() {
                return Err(anyhow!(
                    "server.http.tls.cert_path muss gesetzt sein, wenn TLS aktiviert ist"
                ));
            }
            if self.key_path.as_os_str().is_empty() {
                return Err(anyhow!(
                    "server.http.tls.key_path muss gesetzt sein, wenn TLS aktiviert ist"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct HttpInfo {
    app_name: String,
    app_version: String,
    host: String,
    port: u16,
}

#[derive(Clone)]
struct HttpState {
    registry: Arc<ServiceRegistry>,
    services: Arc<AppServices>,
    auth: Arc<ControlPlaneAuthorizer>,
    info: HttpInfo,
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
        let control_tokens = cfg
            .security
            .http
            .resolve_control_tokens()
            .map_err(|err| anyhow!(err))?;
        let entries = control_tokens
            .into_iter()
            .map(|token| {
                let role = match token.role.as_str() {
                    "admin" => Role::Admin,
                    "operator" => Role::Operator,
                    "viewer" => Role::Viewer,
                    other => {
                        return Err(anyhow!(
                            "unbekannte Rolle in security.http.control_tokens: {other}"
                        ))
                    }
                };
                Ok((role, token.secret))
            })
            .collect::<Result<Vec<_>>>()?;
        if entries.is_empty() {
            return Err(anyhow!(
                "security.http.control_tokens muss für den HTTP-Transport definiert sein"
            ));
        }
        let auth = Arc::new(ControlPlaneAuthorizer::new(entries));
        let runtime = HttpTlsRuntime::from(&cfg.server.http.tls);
        let tls_provider = if runtime.enabled {
            let provider = Arc::new(HttpTlsProvider::new(runtime.clone())?);
            provider.init_watchers()?;
            provider.spawn_auto_reload();
            Some(provider)
        } else {
            None
        };

        Ok(Self {
            config,
            registry,
            services,
            auth,
            handle: Mutex::new(None),
            tls_provider: RwLock::new(tls_provider),
        })
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
                    info!("HTTP TLS aktiviert und Zertifikate geladen");
                }
            } else {
                *guard = None;
                info!("HTTP TLS deaktiviert");
            }
        }

        if let Some((provider, runtime)) = maybe_update {
            provider.update_runtime(runtime).await?;
            info!("HTTP TLS-Konfiguration neu geladen");
        }
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.handle
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    pub async fn start(self: &Arc<Self>) -> Result<bool> {
        {
            if self.is_running() {
                debug!("HTTP server already running");
                return Ok(false);
            }
        }
        self.registry.set_status(
            HTTP_SERVICE_ID,
            ServiceStatus::Starting,
            Some("initialisiere".to_string()),
        );

        let mut resolved = lookup_host((self.config.host.as_str(), self.config.port))
            .await
            .with_context(|| {
                format!(
                    "konnte HTTP-Adresse nicht auflösen: {}:{}",
                    self.config.host, self.config.port
                )
            })?;
        let addr = resolved
            .next()
            .ok_or_else(|| anyhow!("keine Adresse für HTTP-Server gefunden"))?;
        let listener = TcpListener::bind(addr)
            .await
            .with_context(|| format!("HTTP-Server konnte nicht binden: {addr}"))?;
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
            "beende (force)".to_string()
        } else {
            "fahre herunter".to_string()
        };
        self.registry
            .set_status(HTTP_SERVICE_ID, ServiceStatus::Degraded, Some(message));
        match handle.shutdown(force).await {
            Ok(_) => {
                self.registry.set_status(
                    HTTP_SERVICE_ID,
                    ServiceStatus::Stopped,
                    Some(if force {
                        "gestoppt (force)".to_string()
                    } else {
                        "gestoppt".to_string()
                    }),
                );
                info!("HTTP server stopped");
                Ok(true)
            }
            Err(err) => {
                self.registry.set_status(
                    HTTP_SERVICE_ID,
                    ServiceStatus::Failed,
                    Some(format!("Fehler beim Stoppen: {err}")),
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
                Some(format!("Fehler: {err}")),
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
                                    tracing::error!(address = %addr, error = %err, "TLS-Verbindung fehlgeschlagen");
                                }
                            });
                        }
                        Err(err) => {
                            tracing::error!(error = %err, "Fehler beim Annehmen der TLS-Verbindung");
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

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    live: bool,
    ready: bool,
}

#[derive(Serialize)]
struct ServicesResponse {
    services: Vec<ServiceSummary>,
}

#[derive(Serialize)]
struct ServiceStateEvent {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    status: &'static str,
    note: Option<String>,
    since_seconds: Option<u64>,
    critical: bool,
    tags: Vec<&'static str>,
}

#[derive(Clone, Serialize)]
struct ServiceSummary {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    status: &'static str,
    since_seconds: Option<u64>,
    description: &'static str,
    note: Option<String>,
    critical: bool,
    tags: Vec<&'static str>,
}

#[derive(Serialize)]
struct MetricsResponse {
    uptime_seconds: Option<u64>,
    live: bool,
    ready: bool,
    counters: std::collections::BTreeMap<String, u64>,
}

#[derive(Deserialize)]
struct MetricsHistoryQuery {
    range: Option<String>,
}

#[derive(Deserialize, Default)]
struct AuditHistoryQuery {
    range: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct TelemetryHistoryResponse {
    range: String,
    samples: Vec<TelemetryHistorySampleView>,
}

#[derive(Serialize)]
struct AuditHistoryResponse {
    range: String,
    events: Vec<AuditEventView>,
}

#[derive(Serialize)]
struct TelemetryHistorySampleView {
    timestamp_ms: i64,
    timestamp: String,
    metrics: HashMap<String, u64>,
}

#[derive(Serialize)]
struct InfoResponse {
    app: AppInfo,
    http: HttpEndpoint,
}

#[derive(Serialize)]
struct SchedulerJobsResponse {
    jobs: Vec<SchedulerJobSummary>,
}

#[derive(Serialize)]
struct AuditEventsResponse {
    events: Vec<AuditEventView>,
}

#[derive(Serialize)]
struct AuditEventView {
    timestamp: String,
    action: String,
    target: String,
    outcome: String,
    actor: AuditActorView,
    metadata: Vec<AuditMetadataView>,
}

#[derive(Serialize)]
struct AuditActorView {
    kind: &'static str,
    role: Option<String>,
    user_id: Option<String>,
    user_id_redacted: bool,
}

#[derive(Serialize)]
struct AuditMetadataView {
    key: String,
    value: String,
    redacted: bool,
}

#[derive(Deserialize, Default)]
struct AuditQuery {
    limit: Option<usize>,
    action: Option<String>,
    outcome: Option<String>,
    actor: Option<String>,
}

#[derive(Deserialize, Default)]
struct AuditStreamQuery {
    token: Option<String>,
}

#[derive(Serialize)]
struct SchedulerJobSummary {
    id: String,
    interval_seconds: u64,
    description: String,
    active: bool,
}

#[derive(Deserialize)]
struct LoggingLevelRequest {
    level: String,
}

#[derive(Serialize)]
struct LoggingLevelResponse {
    level: String,
    actor_role: &'static str,
}

#[derive(Serialize)]
struct AppInfo {
    name: String,
    version: String,
}

#[derive(Serialize)]
struct HttpEndpoint {
    host: String,
    port: u16,
    base_url: String,
}

#[derive(Deserialize)]
struct ServiceActionPayload {
    #[serde(default)]
    force: bool,
}

#[derive(Deserialize)]
struct BulkActionPayload {
    #[serde(default)]
    force: bool,
}

#[derive(Serialize)]
struct ServiceActionResponse {
    id: String,
    action: &'static str,
    outcome: &'static str,
    force: bool,
    service: Option<ServiceSummary>,
    actor_role: &'static str,
}

#[derive(Serialize)]
struct BulkServiceActionResponse {
    action: &'static str,
    actor_role: &'static str,
    results: Vec<BulkServiceActionItem>,
}

#[derive(Serialize)]
struct BulkServiceActionItem {
    id: String,
    status: &'static str,
    outcome: Option<&'static str>,
    error: Option<String>,
}

#[derive(Serialize)]
struct ServiceActionErrorBody {
    error: &'static str,
    message: String,
}

struct ServiceActionProblem {
    status: StatusCode,
    body: ServiceActionErrorBody,
}

#[derive(Deserialize)]
struct ModuleSearchQueryParams {
    pattern: Option<String>,
}

#[derive(Deserialize)]
struct ModuleInstallRequest {
    module_id: String,
    version: Option<String>,
}

#[derive(Deserialize)]
struct ModuleUpdateRequest {
    module_id: Option<String>,
    fenrir_version: Option<String>,
}

#[derive(Serialize)]
struct InstalledModuleView {
    manifest: ModuleManifest,
    installed_at: Option<String>,
    path: String,
}

#[derive(Serialize)]
struct InstalledModulesResponse {
    modules: Vec<InstalledModuleView>,
}

#[derive(Serialize)]
struct RegistryModuleView {
    id: String,
    version: String,
    title: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
}

#[derive(Serialize)]
struct RegistryModulesResponse {
    modules: Vec<RegistryModuleView>,
}

#[derive(Serialize)]
struct ModuleInstallResponse {
    status: &'static str,
    manifest: ModuleManifest,
    path: String,
}

#[derive(Serialize)]
struct ModuleUpdateResponse {
    results: Vec<ModuleInstallResponse>,
}

enum ServiceActionKind {
    Start,
    Stop,
    Restart,
}

impl ServiceActionKind {
    fn as_str(&self) -> &'static str {
        match self {
            ServiceActionKind::Start => "start",
            ServiceActionKind::Stop => "stop",
            ServiceActionKind::Restart => "restart",
        }
    }

    fn required_role(&self) -> Role {
        match self {
            ServiceActionKind::Start => Role::Operator,
            ServiceActionKind::Stop => Role::Operator,
            ServiceActionKind::Restart => Role::Admin,
        }
    }
}

enum BulkServiceActionKind {
    Start,
    Stop,
    Restart,
}

impl BulkServiceActionKind {
    fn as_str(&self) -> &'static str {
        match self {
            BulkServiceActionKind::Start => "start-all",
            BulkServiceActionKind::Stop => "stop-all",
            BulkServiceActionKind::Restart => "restart-all",
        }
    }

    fn required_role(&self) -> Role {
        match self {
            BulkServiceActionKind::Start => Role::Operator,
            BulkServiceActionKind::Stop => Role::Operator,
            BulkServiceActionKind::Restart => Role::Admin,
        }
    }

    fn execute(
        &self,
        services: &AppServices,
        force: bool,
    ) -> Vec<crate::services::ServiceActionReport> {
        match self {
            BulkServiceActionKind::Start => services.start_all_non_core(),
            BulkServiceActionKind::Stop => services.stop_all_non_core(force),
            BulkServiceActionKind::Restart => services.restart_all_non_core(force),
        }
    }
}

impl ServiceActionProblem {
    fn new(status: StatusCode, error: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ServiceActionErrorBody {
                error,
                message: message.into(),
            },
        }
    }
}

impl IntoResponse for ServiceActionProblem {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

fn snapshot_to_summary(svc: ServiceSnapshot) -> ServiceSummary {
    ServiceSummary {
        id: svc.descriptor.id,
        name: svc.descriptor.name,
        kind: svc.descriptor.kind.as_str(),
        status: svc.status.label(),
        since_seconds: svc.since.elapsed().ok().map(|duration| duration.as_secs()),
        description: svc.descriptor.description,
        note: svc.note,
        critical: svc.descriptor.critical,
        tags: svc.descriptor.tags.iter().map(|tag| tag.as_str()).collect(),
    }
}

fn snapshot_to_state_event(snapshot: ServiceSnapshot) -> ServiceStateEvent {
    ServiceStateEvent {
        id: snapshot.descriptor.id,
        name: snapshot.descriptor.name,
        kind: snapshot.descriptor.kind.as_str(),
        status: snapshot.status.label(),
        note: snapshot.note,
        since_seconds: snapshot
            .since
            .elapsed()
            .ok()
            .map(|duration| duration.as_secs()),
        critical: snapshot.descriptor.critical,
        tags: snapshot
            .descriptor
            .tags
            .iter()
            .map(|tag| tag.as_str())
            .collect(),
    }
}

fn module_service_unavailable() -> ServiceActionProblem {
    ServiceActionProblem::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "module_service_unavailable",
        "Modul-Service nicht verfügbar",
    )
}

fn system_time_to_rfc3339(time: SystemTime) -> Option<String> {
    OffsetDateTime::from(time).format(&Rfc3339).ok()
}

fn installed_module_to_view(installed: InstalledModule) -> InstalledModuleView {
    InstalledModuleView {
        manifest: installed.manifest,
        installed_at: system_time_to_rfc3339(installed.installed_at),
        path: installed.path,
    }
}

fn module_install_status_label(status: ModuleInstallStatus) -> &'static str {
    match status {
        ModuleInstallStatus::Installed => "installed",
        ModuleInstallStatus::Updated => "updated",
        ModuleInstallStatus::AlreadyCurrent => "already_current",
    }
}

fn map_install_result(result: ModuleInstallResult) -> ModuleInstallResponse {
    ModuleInstallResponse {
        status: module_install_status_label(result.status),
        manifest: result.manifest,
        path: result.path,
    }
}

fn module_error_problem(err: ModuleServiceError) -> ServiceActionProblem {
    match err {
        ModuleServiceError::Registry(inner) => match inner {
            ModuleRegistryError::Unavailable(msg) => {
                ServiceActionProblem::new(StatusCode::BAD_GATEWAY, "registry_unavailable", msg)
            }
            ModuleRegistryError::NotFound { module } => ServiceActionProblem::new(
                StatusCode::NOT_FOUND,
                "module_not_found",
                format!("Modul '{}' wurde nicht gefunden", module),
            ),
            ModuleRegistryError::Protocol(msg) => {
                ServiceActionProblem::new(StatusCode::BAD_GATEWAY, "registry_protocol_error", msg)
            }
        },
        ModuleServiceError::Storage(inner) => match inner {
            ModuleStorageError::Unavailable(msg) | ModuleStorageError::Io(msg) => {
                ServiceActionProblem::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "module_storage_error",
                    msg,
                )
            }
            ModuleStorageError::InvalidState(msg) => {
                ServiceActionProblem::new(StatusCode::CONFLICT, "module_invalid_state", msg)
            }
        },
        ModuleServiceError::Verification(inner) => ServiceActionProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "module_verification_error",
            inner.to_string(),
        ),
    }
}

fn module_validation_problem(code: &'static str, err: ModuleError) -> ServiceActionProblem {
    match err {
        ModuleError::Validation(msg) => {
            ServiceActionProblem::new(StatusCode::BAD_REQUEST, code, msg)
        }
    }
}

async fn list_installed_modules(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Viewer) {
        return problem.into_response();
    }

    let Some(service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };

    match service.list_installed().await {
        Ok(modules) => {
            let views = modules.into_iter().map(installed_module_to_view).collect();
            (
                StatusCode::OK,
                Json(InstalledModulesResponse { modules: views }),
            )
                .into_response()
        }
        Err(err) => module_error_problem(err).into_response(),
    }
}

async fn list_available_modules(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<ModuleSearchQueryParams>,
) -> Response {
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Viewer) {
        return problem.into_response();
    }

    let Some(service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };

    let search_query = ModuleSearchQuery::new(query.pattern.clone());

    match service.search(search_query).await {
        Ok(entries) => {
            let modules = entries
                .into_iter()
                .map(|entry| RegistryModuleView {
                    id: entry.id.to_string(),
                    version: entry.version.to_string(),
                    title: entry.title,
                    description: entry.description,
                    tags: entry.tags,
                })
                .collect();
            (StatusCode::OK, Json(RegistryModulesResponse { modules })).into_response()
        }
        Err(err) => module_error_problem(err).into_response(),
    }
}

async fn install_module_version(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(payload): Json<ModuleInstallRequest>,
) -> Response {
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Operator) {
        return problem.into_response();
    }

    let Some(service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };

    let module_id = match ModuleId::new(payload.module_id.trim()) {
        Ok(id) => id,
        Err(err) => return module_validation_problem("invalid_module_id", err).into_response(),
    };

    let maybe_version = match payload.version.as_deref() {
        Some(raw) => match ModuleVersion::parse(raw) {
            Ok(version) => Some(version),
            Err(err) => {
                return module_validation_problem("invalid_module_version", err).into_response()
            }
        },
        None => None,
    };

    match service.install(&module_id, maybe_version.as_ref()).await {
        Ok(result) => (StatusCode::OK, Json(map_install_result(result))).into_response(),
        Err(err) => module_error_problem(err).into_response(),
    }
}

async fn update_module_versions(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(payload): Json<ModuleUpdateRequest>,
) -> Response {
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Operator) {
        return problem.into_response();
    }

    let Some(service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };

    let fenrir_version_owned = payload
        .fenrir_version
        .clone()
        .or_else(|| Some(state.info.app_version.clone()));
    let fenrir_version = fenrir_version_owned.as_deref();

    let mut results = Vec::new();

    if let Some(module_id_raw) = payload.module_id.as_deref() {
        let module_id = match ModuleId::new(module_id_raw.trim()) {
            Ok(id) => id,
            Err(err) => return module_validation_problem("invalid_module_id", err).into_response(),
        };

        match service.update(&module_id, fenrir_version).await {
            Ok(result) => results.push(map_install_result(result)),
            Err(err) => return module_error_problem(err).into_response(),
        }
    } else {
        match service.update_all(fenrir_version).await {
            Ok(items) => {
                results.extend(items.into_iter().map(map_install_result));
            }
            Err(err) => return module_error_problem(err).into_response(),
        }
    }

    (StatusCode::OK, Json(ModuleUpdateResponse { results })).into_response()
}

async fn uninstall_module_version(
    State(state): State<HttpState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Operator) {
        return problem.into_response();
    }

    let Some(service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };

    let module_id = match ModuleId::new(id.trim()) {
        Ok(id) => id,
        Err(err) => return module_validation_problem("invalid_module_id", err).into_response(),
    };

    match service.uninstall(&module_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => module_error_problem(err).into_response(),
    }
}

fn build_router(state: HttpState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/info", get(info))
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/services", get(list_services))
        .route("/services/:id/start", post(start_service))
        .route("/services/:id/stop", post(stop_service))
        .route("/services/:id/restart", post(restart_service))
        .route("/services/actions/start-all", post(start_all_services))
        .route("/services/actions/stop-all", post(stop_all_services))
        .route("/services/actions/restart-all", post(restart_all_services))
        .route("/scheduler/jobs", get(list_scheduler_jobs))
        .route("/logging/level", post(update_logging_level))
        .route("/metrics", get(metrics_snapshot))
        .route("/metrics/history", get(metrics_history))
        .route("/audit/history", get(audit_history))
        .route("/audit", get(list_audit_events))
        .route("/events/stream", get(audit_events_stream))
        .route("/modules/installed", get(list_installed_modules))
        .route("/modules/available", get(list_available_modules))
        .route("/modules/install", post(install_module_version))
        .route("/modules/update", post(update_module_versions))
        .route("/modules/:id", delete(uninstall_module_version))
        .with_state(state)
}

pub fn router_with_dependencies(
    registry: Arc<ServiceRegistry>,
    services: Arc<AppServices>,
    auth: Arc<ControlPlaneAuthorizer>,
    app_name: impl Into<String>,
    app_version: impl Into<String>,
    host: impl Into<String>,
    port: u16,
) -> Router {
    let state = HttpState {
        registry,
        services,
        auth,
        info: HttpInfo {
            app_name: app_name.into(),
            app_version: app_version.into(),
            host: host.into(),
            port,
        },
    };
    build_router(state)
}

pub fn admin_console_html() -> &'static str {
    INDEX_HTML
}

async fn index() -> impl IntoResponse {
    Html(INDEX_HTML)
}

async fn info(State(state): State<HttpState>) -> impl IntoResponse {
    let info = state.info.clone();
    Json(InfoResponse {
        app: AppInfo {
            name: info.app_name.clone(),
            version: info.app_version.clone(),
        },
        http: HttpEndpoint {
            host: info.host.clone(),
            port: info.port,
            base_url: format!("http://{}:{}", info.host, info.port),
        },
    })
}

async fn health_live() -> impl IntoResponse {
    let live = telemetry::is_live();
    let body = HealthResponse {
        status: if live { "ok" } else { "unavailable" },
        live,
        ready: telemetry::is_ready(),
    };
    let status = if live {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body))
}

async fn health_ready() -> impl IntoResponse {
    let ready = telemetry::is_ready();
    let live = telemetry::is_live();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = HealthResponse {
        status: if ready { "ok" } else { "unavailable" },
        live,
        ready,
    };
    (status, Json(body))
}

async fn list_services(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if state.auth.is_configured() {
        let token = extract_bearer_token(&headers);
        let role = state.auth.authorize_token(token).map_err(map_auth_error);
        match role {
            Ok(role) if role.satisfies(Role::Viewer) => {}
            Ok(_) => {
                return ServiceActionProblem::new(
                    StatusCode::FORBIDDEN,
                    "role_insufficient",
                    "Mindestens Rolle viewer erforderlich",
                )
                .into_response()
            }
            Err(problem) => return problem.into_response(),
        }
    }
    let mut services: Vec<ServiceSummary> = state
        .registry
        .snapshot()
        .into_iter()
        .map(snapshot_to_summary)
        .collect();
    services.sort_by(|a, b| a.id.cmp(&b.id));
    (StatusCode::OK, Json(ServicesResponse { services })).into_response()
}

async fn list_scheduler_jobs(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if state.auth.is_configured() {
        let token = extract_bearer_token(&headers);
        let role = state.auth.authorize_token(token).map_err(map_auth_error);
        match role {
            Ok(role) if role.satisfies(Role::Viewer) => {}
            Ok(_) => {
                return ServiceActionProblem::new(
                    StatusCode::FORBIDDEN,
                    "role_insufficient",
                    "Mindestens Rolle viewer erforderlich",
                )
                .into_response()
            }
            Err(problem) => return problem.into_response(),
        }
    }

    let jobs: Vec<SchedulerJobSummary> = state
        .services
        .scheduler_service()
        .jobs()
        .into_iter()
        .map(|job: ScheduledJobSnapshot| SchedulerJobSummary {
            id: job.id,
            interval_seconds: job.interval.as_secs(),
            description: job.description,
            active: job.active,
        })
        .collect();

    (StatusCode::OK, Json(SchedulerJobsResponse { jobs })).into_response()
}

async fn list_audit_events(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Response {
    if state.auth.is_configured() {
        let token = extract_bearer_token(&headers);
        let role = state.auth.authorize_token(token).map_err(map_auth_error);
        match role {
            Ok(role) if role.satisfies(Role::Viewer) => {}
            Ok(_) => {
                return ServiceActionProblem::new(
                    StatusCode::FORBIDDEN,
                    "role_insufficient",
                    "Mindestens Rolle viewer erforderlich",
                )
                .into_response()
            }
            Err(problem) => return problem.into_response(),
        }
    }

    let limit = query.limit.unwrap_or(20).max(1).min(200);

    match state.services.audit_recent(limit) {
        Ok(events) => {
            let filtered: Vec<AuditEventView> = events
                .into_iter()
                .filter(|event| {
                    if let Some(ref action) = query.action {
                        if event.action != *action {
                            return false;
                        }
                    }
                    if let Some(ref outcome) = query.outcome {
                        if !audit_outcome_matches(&event.outcome, outcome) {
                            return false;
                        }
                    }
                    if let Some(ref actor) = query.actor {
                        if !audit_actor_matches(&event.actor, actor) {
                            return false;
                        }
                    }
                    true
                })
                .map(audit_event_to_view)
                .collect();
            (
                StatusCode::OK,
                Json(AuditEventsResponse { events: filtered }),
            )
                .into_response()
        }
        Err(err) => ServiceActionProblem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "audit_unavailable",
            format!("Audit-Store nicht verfügbar: {err}"),
        )
        .into_response(),
    }
}

async fn audit_events_stream(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AuditStreamQuery>,
) -> Response {
    if state.auth.is_configured() {
        let header_token = extract_bearer_token(&headers).map(|value| value.to_string());
        let token = header_token.or(query.token.clone());
        match state.auth.authorize_token(token.as_deref()) {
            Ok(role) if role.satisfies(Role::Viewer) => {}
            Ok(_) => {
                return ServiceActionProblem::new(
                    StatusCode::FORBIDDEN,
                    "role_insufficient",
                    "Mindestens Rolle viewer erforderlich",
                )
                .into_response();
            }
            Err(err) => {
                return map_auth_error(err).into_response();
            }
        }
    }

    let audit_receiver = state.services.audit_subscribe();
    let audit_stream = BroadcastStream::new(audit_receiver).filter_map(|result| match result {
        Ok(event) => match serde_json::to_string(&audit_event_to_view(event)) {
            Ok(json) => Some(Ok::<Event, Infallible>(
                Event::default().event("audit").data(json),
            )),
            Err(err) => {
                warn!(error = %err, "failed to encode audit event for sse");
                None
            }
        },
        Err(BroadcastStreamRecvError::Lagged(_)) => None,
    });

    let service_receiver = state.registry.subscribe();
    let service_stream = BroadcastStream::new(service_receiver).filter_map(|result| match result {
        Ok(snapshot) => match serde_json::to_string(&snapshot_to_state_event(snapshot)) {
            Ok(json) => Some(Ok::<Event, Infallible>(
                Event::default().event("service-state").data(json),
            )),
            Err(err) => {
                warn!(error = %err, "failed to encode service event for sse");
                None
            }
        },
        Err(BroadcastStreamRecvError::Lagged(_)) => None,
    });

    let stream = audit_stream.merge(service_stream);

    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

async fn metrics_snapshot() -> impl IntoResponse {
    let snapshot = telemetry::snapshot();
    let body = MetricsResponse {
        uptime_seconds: snapshot.as_ref().map(|s| s.uptime.as_secs()),
        live: telemetry::is_live(),
        ready: telemetry::is_ready(),
        counters: snapshot
            .map(|s| s.metrics.into_iter().collect())
            .unwrap_or_default(),
    };
    Json(body)
}

async fn metrics_history(Query(query): Query<MetricsHistoryQuery>) -> impl IntoResponse {
    let (range, label) = parse_history_range(query.range.as_deref());
    let samples = telemetry::history(range);
    let views: Vec<TelemetryHistorySampleView> =
        samples.into_iter().map(history_sample_to_view).collect();
    Json(TelemetryHistoryResponse {
        range: label.to_string(),
        samples: views,
    })
}

async fn audit_history(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AuditHistoryQuery>,
) -> Response {
    if state.auth.is_configured() {
        let token = extract_bearer_token(&headers);
        let role = state.auth.authorize_token(token).map_err(map_auth_error);
        match role {
            Ok(role) if role.satisfies(Role::Viewer) => {}
            Ok(_) => {
                return ServiceActionProblem::new(
                    StatusCode::FORBIDDEN,
                    "role_insufficient",
                    "Mindestens Rolle viewer erforderlich",
                )
                .into_response();
            }
            Err(problem) => return problem.into_response(),
        }
    }

    let (range, label) = parse_history_range(query.range.as_deref());
    let limit = query.limit.unwrap_or(512).clamp(1, 5000);

    match state.services.audit_recent(limit) {
        Ok(events) => {
            let cutoff = SystemTime::now()
                .checked_sub(range)
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let filtered: Vec<AuditEventView> = events
                .into_iter()
                .filter(|event| event.timestamp >= cutoff)
                .map(audit_event_to_view)
                .collect();

            Json(AuditHistoryResponse {
                range: label.to_string(),
                events: filtered,
            })
            .into_response()
        }
        Err(err) => ServiceActionProblem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "audit_unavailable",
            format!("Audit-Store nicht verfügbar: {err}"),
        )
        .into_response(),
    }
}

async fn update_logging_level(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(payload): Json<LoggingLevelRequest>,
) -> Response {
    let level = payload.level.trim();
    if level.is_empty() {
        return ServiceActionProblem::new(
            StatusCode::BAD_REQUEST,
            "invalid_level",
            "Loglevel darf nicht leer sein",
        )
        .into_response();
    }

    let role = if state.auth.is_configured() {
        let token = extract_bearer_token(&headers);
        match state.auth.authorize_token(token) {
            Ok(role) => {
                if !role.satisfies(Role::Admin) {
                    return ServiceActionProblem::new(
                        StatusCode::FORBIDDEN,
                        "role_insufficient",
                        "Aktion erfordert Rolle admin",
                    )
                    .into_response();
                }
                role
            }
            Err(err) => return map_auth_error(err).into_response(),
        }
    } else {
        Role::Admin
    };

    let handle = match state.services.logging_handle() {
        Some(handle) => handle,
        None => {
            return ServiceActionProblem::new(
                StatusCode::NOT_IMPLEMENTED,
                "logging_reload_unavailable",
                "Kein Logging-Reload-Handle registriert",
            )
            .into_response();
        }
    };

    if let Err(err) = logging::reload(&handle, level) {
        return ServiceActionProblem::new(
            StatusCode::BAD_REQUEST,
            "logging_reload_failed",
            format!("Loglevel konnte nicht gesetzt werden: {err}"),
        )
        .into_response();
    }

    let response = LoggingLevelResponse {
        level: level.to_string(),
        actor_role: role.as_str(),
    };
    (StatusCode::OK, Json(response)).into_response()
}

async fn start_service(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    respond_service_action(state, id, ServiceActionKind::Start, false, headers).await
}

async fn stop_service(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    payload: Option<Json<ServiceActionPayload>>,
) -> Response {
    let force = payload.map(|body| body.force).unwrap_or(false);
    respond_service_action(state, id, ServiceActionKind::Stop, force, headers).await
}

async fn restart_service(
    State(state): State<HttpState>,
    AxumPath(id): AxumPath<String>,
    headers: HeaderMap,
    payload: Option<Json<ServiceActionPayload>>,
) -> Response {
    let force = payload.map(|body| body.force).unwrap_or(false);
    respond_service_action(state, id, ServiceActionKind::Restart, force, headers).await
}

async fn start_all_services(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    bulk_service_action(state, BulkServiceActionKind::Start, false, headers).await
}

async fn stop_all_services(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Option<Json<BulkActionPayload>>,
) -> Response {
    let force = payload.map(|body| body.force).unwrap_or(false);
    bulk_service_action(state, BulkServiceActionKind::Stop, force, headers).await
}

async fn restart_all_services(
    State(state): State<HttpState>,
    headers: HeaderMap,
    payload: Option<Json<BulkActionPayload>>,
) -> Response {
    let force = payload.map(|body| body.force).unwrap_or(false);
    bulk_service_action(state, BulkServiceActionKind::Restart, force, headers).await
}

async fn respond_service_action(
    state: HttpState,
    id: String,
    kind: ServiceActionKind,
    force: bool,
    headers: HeaderMap,
) -> Response {
    match service_action(state, id, kind, force, headers).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(problem) => problem.into_response(),
    }
}

async fn service_action(
    state: HttpState,
    id: String,
    kind: ServiceActionKind,
    force: bool,
    headers: HeaderMap,
) -> Result<ServiceActionResponse, ServiceActionProblem> {
    let HttpState {
        registry,
        services,
        auth,
        ..
    } = state;

    let role = authorize(&auth, &services, &headers, kind.required_role())?;
    let actor = audit_actor_from_role(&role);
    let action_label = format!("service::{}", kind.as_str());
    let base_metadata = audit_metadata_base(&role, force);

    let outcome = match kind {
        ServiceActionKind::Start => services.start_service(&id),
        ServiceActionKind::Stop => services.stop_service(&id, force),
        ServiceActionKind::Restart => services.restart_service(&id, force),
    };

    match outcome {
        Ok(control) => {
            let snapshot = registry.get(&id).map(snapshot_to_summary);
            let mut metadata = base_metadata
                .clone()
                .insert("service_outcome", control.as_str());
            if let Some(service) = snapshot.as_ref() {
                metadata = metadata.insert("service_status", service.status);
                if let Some(note) = service.note.as_deref() {
                    metadata = metadata.insert("note", note);
                }
            }
            push_audit_event(
                &services,
                actor,
                &action_label,
                &id,
                AuditOutcome::Success,
                metadata,
            );

            telemetry::record_counter("service.action.total", 1);
            telemetry::record_counter("service.action.success", 1);
            telemetry::record_counter(&format!("service.action.{}.success", kind.as_str()), 1);

            Ok(ServiceActionResponse {
                id,
                action: kind.as_str(),
                outcome: control.as_str(),
                force,
                service: snapshot,
                actor_role: role.as_str(),
            })
        }
        Err(err) => {
            let problem = map_service_control_error(err, &id);
            let metadata = base_metadata
                .clone()
                .insert("error_code", problem.body.error)
                .insert("http_status", problem.status.as_u16().to_string())
                .insert("message", problem.body.message.clone());
            push_audit_event(
                &services,
                actor,
                &action_label,
                &id,
                AuditOutcome::Failure,
                metadata,
            );
            telemetry::record_counter("service.action.total", 1);
            telemetry::record_counter("service.action.failure", 1);
            telemetry::record_counter(&format!("service.action.{}.failure", kind.as_str()), 1);
            Err(problem)
        }
    }
}

async fn bulk_service_action(
    state: HttpState,
    kind: BulkServiceActionKind,
    force: bool,
    headers: HeaderMap,
) -> Response {
    let HttpState {
        registry: _,
        services,
        auth,
        ..
    } = state;

    match authorize(&auth, &services, &headers, kind.required_role()) {
        Ok(role) => {
            let actor = audit_actor_from_role(&role);
            let base_metadata = audit_metadata_base(&role, force);
            let reports = kind.execute(&services, force);
            let mut success_count = 0usize;
            let mut failure_ids: Vec<String> = Vec::new();
            let mut failure_messages: Vec<String> = Vec::new();
            let response = BulkServiceActionResponse {
                action: kind.as_str(),
                actor_role: role.as_str(),
                results: reports
                    .into_iter()
                    .map(|report| match report.result {
                        Ok(outcome) => {
                            success_count += 1;
                            BulkServiceActionItem {
                                id: report.id,
                                status: "success",
                                outcome: Some(outcome.as_str()),
                                error: None,
                            }
                        }
                        Err(err) => {
                            failure_ids.push(report.id.clone());
                            let message = err.to_string();
                            failure_messages.push(message.clone());
                            BulkServiceActionItem {
                                id: report.id,
                                status: "error",
                                outcome: None,
                                error: Some(message),
                            }
                        }
                    })
                    .collect(),
            };
            let total = response.results.len();
            let failed = total.saturating_sub(success_count);
            let mut metadata = base_metadata
                .clone()
                .insert("total", total.to_string())
                .insert("success", success_count.to_string());
            if failed > 0 {
                metadata = metadata
                    .insert("failed", failed.to_string())
                    .insert("failed_ids", failure_ids.join(","));
                if !failure_messages.is_empty() {
                    metadata = metadata.insert("errors", failure_messages.join(" | "));
                }
            }
            let action_label = format!("service::bulk::{}", kind.as_str());
            let outcome = if failed == 0 {
                AuditOutcome::Success
            } else {
                AuditOutcome::Failure
            };

            telemetry::record_counter("service.bulk.total", 1);
            telemetry::record_counter(&format!("service.bulk.{}.total", kind.as_str()), 1);
            telemetry::record_counter("service.bulk.success_services", success_count as u64);
            telemetry::record_counter("service.bulk.failed_services", failed as u64);
            telemetry::record_counter(
                &format!("service.bulk.{}.success_services", kind.as_str()),
                success_count as u64,
            );
            telemetry::record_counter(
                &format!("service.bulk.{}.failed_services", kind.as_str()),
                failed as u64,
            );
            push_audit_event(
                &services,
                actor,
                &action_label,
                kind.as_str(),
                outcome,
                metadata,
            );
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(problem) => problem.into_response(),
    }
}

fn map_service_control_error(err: ServiceControlError, id: &str) -> ServiceActionProblem {
    match err {
        ServiceControlError::UnknownService(_) => ServiceActionProblem::new(
            StatusCode::NOT_FOUND,
            "unknown_service",
            format!("Service `{}` ist nicht registriert.", id),
        ),
        ServiceControlError::NotControllable(_) => ServiceActionProblem::new(
            StatusCode::CONFLICT,
            "not_controllable",
            format!("Service `{}` lässt sich nicht über HTTP steuern.", id),
        ),
        ServiceControlError::ForceRequired(_) => ServiceActionProblem::new(
            StatusCode::PRECONDITION_FAILED,
            "force_required",
            format!(
                "Service `{}` ist als kritisch markiert. Bitte Aktion mit force=true bestätigen.",
                id
            ),
        ),
        ServiceControlError::CoreLocked(_) => ServiceActionProblem::new(
            StatusCode::CONFLICT,
            "core_locked",
            format!(
                "Service `{}` gehört zur core-Plattform und kann nicht gestoppt oder neu gestartet werden.",
                id
            ),
        ),
        ServiceControlError::OperationFailed { source, .. } => ServiceActionProblem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "operation_failed",
            format!("Aktion fehlgeschlagen: {source}"),
        ),
    }
}

fn audit_actor_from_role(role: &Role) -> AuditActor {
    AuditActor::User {
        user_id: format!("http::{}", role.as_str()),
        role: role.as_str().to_string(),
    }
}

fn audit_metadata_base(role: &Role, force: bool) -> AuditMetadata {
    AuditMetadata::default()
        .insert("transport", "http")
        .insert("actor_role", role.as_str())
        .insert("force", if force { "true" } else { "false" })
}

fn push_audit_event(
    services: &AppServices,
    actor: AuditActor,
    action: &str,
    target: &str,
    outcome: AuditOutcome,
    metadata: AuditMetadata,
) {
    match AuditEvent::builder()
        .actor(actor)
        .action(action.to_string())
        .target(target.to_string())
        .outcome(outcome)
        .metadata(metadata)
        .build()
    {
        Ok(event) => {
            if let Err(err) = services.record_audit(event) {
                warn!(error = %err, "failed to append audit event");
            }
        }
        Err(err) => warn!(error = %err, "failed to build audit event"),
    }
}

fn audit_outcome_matches(outcome: &AuditOutcome, filter: &str) -> bool {
    let normalized = filter.to_ascii_lowercase();
    match outcome {
        AuditOutcome::Success => normalized == "success",
        AuditOutcome::Failure => normalized == "failure",
        AuditOutcome::Denied => normalized == "denied",
    }
}

fn audit_actor_matches(actor: &AuditActor, filter: &str) -> bool {
    match actor {
        AuditActor::System => filter.eq_ignore_ascii_case("system"),
        AuditActor::User { user_id, role } => {
            filter.eq_ignore_ascii_case(role) || user_id.eq_ignore_ascii_case(filter)
        }
    }
}

fn audit_event_to_view(event: AuditEvent) -> AuditEventView {
    let timestamp = OffsetDateTime::from(event.timestamp);
    let ts = timestamp
        .format(&Rfc3339)
        .unwrap_or_else(|_| timestamp.to_string());

    let actor_view = match event.actor {
        AuditActor::System => AuditActorView {
            kind: "system",
            role: None,
            user_id: None,
            user_id_redacted: false,
        },
        AuditActor::User { user_id, role } => {
            let redacted = event.redactions.iter().any(|field| field == "user_id");
            AuditActorView {
                kind: "user",
                role: Some(role),
                user_id: if redacted { None } else { Some(user_id) },
                user_id_redacted: redacted,
            }
        }
    };

    let metadata = event
        .metadata
        .as_slice()
        .iter()
        .map(|(key, value)| {
            let redacted = event.redactions.iter().any(|field| field == key);
            AuditMetadataView {
                key: key.clone(),
                value: if redacted {
                    "<redacted>".to_string()
                } else {
                    value.clone()
                },
                redacted,
            }
        })
        .collect();

    AuditEventView {
        timestamp: ts,
        action: event.action,
        target: event.target,
        outcome: event.outcome.to_string(),
        actor: actor_view,
        metadata,
    }
}

fn map_auth_error(err: AuthError) -> ServiceActionProblem {
    match err {
        AuthError::Unauthorized => ServiceActionProblem::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Autorisierung erforderlich",
        ),
        AuthError::Forbidden => {
            ServiceActionProblem::new(StatusCode::FORBIDDEN, "forbidden", "Zugriff verweigert")
        }
    }
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    let header_value = headers.get(AUTHORIZATION)?.to_str().ok()?.trim();
    if let Some(token) = header_value.strip_prefix("Bearer ") {
        if !token.is_empty() {
            return Some(token);
        }
    } else if !header_value.is_empty() {
        return Some(header_value);
    }
    None
}

fn authorize(
    auth: &ControlPlaneAuthorizer,
    services: &AppServices,
    headers: &HeaderMap,
    required: Role,
) -> Result<Role, ServiceActionProblem> {
    let token = extract_bearer_token(headers);
    match auth.authorize_token(token) {
        Ok(role) => {
            if role.satisfies(required.clone()) {
                if let Some(security) = services.security_manager() {
                    security.audit_control_plane_token(token, required.clone(), &Ok(role.clone()));
                }
                Ok(role)
            } else {
                if let Some(security) = services.security_manager() {
                    security.audit_control_plane_token(
                        token,
                        required.clone(),
                        &Err(AuthError::Forbidden),
                    );
                }
                Err(ServiceActionProblem::new(
                    StatusCode::FORBIDDEN,
                    "role_insufficient",
                    format!(
                        "Aktion erfordert Rolle {required:?}, aktuelle Rolle {role:?} reicht nicht aus"
                    ),
                ))
            }
        }
        Err(err) => {
            if let Some(security) = services.security_manager() {
                let audit_err = match &err {
                    AuthError::Unauthorized => AuthError::Unauthorized,
                    AuthError::Forbidden => AuthError::Forbidden,
                };
                security.audit_control_plane_token(token, required.clone(), &Err(audit_err));
            }
            Err(map_auth_error(err))
        }
    }
}

fn parse_history_range(range: Option<&str>) -> (Duration, &'static str) {
    match range.unwrap_or("1h") {
        "3h" => (Duration::from_secs(3 * 3600), "3h"),
        "24h" | "1d" | "day" => (Duration::from_secs(24 * 3600), "24h"),
        _ => (Duration::from_secs(3600), "1h"),
    }
}

fn history_sample_to_view(sample: telemetry::MetricHistoryPoint) -> TelemetryHistorySampleView {
    TelemetryHistorySampleView {
        timestamp_ms: sample.timestamp_ms,
        timestamp: history_timestamp_iso(sample.timestamp_ms),
        metrics: sample.metrics,
    }
}

fn history_timestamp_iso(timestamp_ms: i64) -> String {
    let secs = timestamp_ms.div_euclid(1000);
    let millis = timestamp_ms.rem_euclid(1000);
    match OffsetDateTime::from_unix_timestamp(secs) {
        Ok(dt) => match dt.checked_add(TimeDuration::milliseconds(millis as i64)) {
            Some(adjusted) => adjusted
                .format(&Rfc3339)
                .unwrap_or_else(|_| timestamp_ms.to_string()),
            None => timestamp_ms.to_string(),
        },
        Err(_) => timestamp_ms.to_string(),
    }
}

struct HttpTlsProvider {
    runtime: RwLock<HttpTlsRuntime>,
    config: ArcSwap<RustlsServerConfig>,
    next_reload: Mutex<Option<Instant>>,
    watcher: Mutex<Option<TlsFileWatcher>>,
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
    fn new(runtime: HttpTlsRuntime) -> Result<Self> {
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
        };
        Ok(provider)
    }

    fn init_watchers(self: &Arc<Self>) -> Result<()> {
        let runtime = self
            .runtime
            .read()
            .map_err(|_| anyhow!("tls runtime lock poisoned"))?
            .clone();
        if runtime.enabled {
            self.start_watcher(runtime)?;
        }
        Ok(())
    }

    fn restart_watcher(self: &Arc<Self>, runtime: HttpTlsRuntime) -> Result<()> {
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
                "TLS-Zertifikat '{}' wurde nicht gefunden",
                cert_path.display()
            ));
        }
        if !key_path.exists() {
            return Err(anyhow!(
                "TLS-Schlüssel '{}' wurde nicht gefunden",
                key_path.display()
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
                        tracing::error!(error = %err, "TLS-Datei-Watcher konnte nicht erstellt werden");
                        return;
                    }
                };

                if let Err(err) = watcher.watch(&cert_path, RecursiveMode::NonRecursive) {
                    tracing::error!(
                        path = %cert_path.display(),
                        error = %err,
                        "TLS-Zertifikat kann nicht beobachtet werden"
                    );
                    return;
                }
                if key_path != cert_path {
                    if let Err(err) = watcher.watch(&key_path, RecursiveMode::NonRecursive) {
                        tracing::error!(
                            path = %key_path.display(),
                            error = %err,
                            "TLS-Schlüssel kann nicht beobachtet werden"
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
                                    if let Err(err) = provider.refresh_sync() {
                                        tracing::warn!(
                                            error = %err,
                                            "TLS-Zertifikate konnten nicht neu geladen werden"
                                        );
                                    } else {
                                        tracing::info!("TLS-Zertifikate neu geladen (Filesystem-Event)");
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        Ok(Err(err)) => {
                            tracing::warn!(error = %err, "Fehler beim Beobachten der TLS-Artefakte");
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;

        let mut guard = self
            .watcher
            .lock()
            .map_err(|_| anyhow!("tls watcher lock poisoned"))?;
        *guard = Some(TlsFileWatcher {
            shutdown: shutdown_tx,
            thread: handle,
        });
        Ok(())
    }

    fn stop_watcher(&self) {
        if let Ok(mut guard) = self.watcher.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.shutdown.send(());
                let _ = handle.thread.join();
            }
        }
    }

    fn refresh_sync(&self) -> Result<()> {
        let runtime = self
            .runtime
            .read()
            .map_err(|_| anyhow!("tls runtime lock poisoned"))?
            .clone();
        let config = load_server_config_sync(&runtime)?;
        self.config.store(Arc::new(config));
        if let Ok(mut guard) = self.next_reload.lock() {
            *guard = runtime
                .reload_interval
                .map(|interval| Instant::now() + interval);
        }
        Ok(())
    }

    fn spawn_auto_reload(self: &Arc<Self>) {
        let interval = {
            let runtime = self.runtime.read().expect("tls runtime lock");
            runtime.reload_interval
        };
        if let Some(interval) = interval {
            let this = Arc::clone(self);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    if let Err(err) = this.reload_now().await {
                        tracing::warn!(error = %err, "TLS-Zertifikate konnten nicht neu geladen werden");
                    }
                }
            });
        }
    }

    async fn serve_connection(
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
                        tracing::error!(error = %err, "TLS request handling failed");
                        let body = axum::body::Body::from("internal server error");
                        let response = HyperResponse::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
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
        // Adapt Tower service to Hyper service
        let svc = TowerToHyperService::new(tower_svc);
        builder
            .serve_connection(io, svc)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    async fn reload_now(&self) -> Result<()> {
        let runtime = self
            .runtime
            .read()
            .map_err(|_| anyhow!("tls runtime lock poisoned"))?
            .clone();
        let config = load_server_config_async(&runtime).await?;
        self.config.store(Arc::new(config));
        if let Ok(mut guard) = self.next_reload.lock() {
            *guard = runtime
                .reload_interval
                .map(|interval| Instant::now() + interval);
        }
        Ok(())
    }

    async fn reload_if_due(&self) -> Result<()> {
        let reload_due = {
            let mut guard = self
                .next_reload
                .lock()
                .map_err(|_| anyhow!("tls reload lock poisoned"))?;
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
            self.reload_now().await?;
        }
        Ok(())
    }

    async fn update_runtime(self: &Arc<Self>, runtime: HttpTlsRuntime) -> Result<()> {
        runtime.validate()?;
        {
            let mut guard = self
                .runtime
                .write()
                .map_err(|_| anyhow!("tls runtime lock poisoned"))?;
            *guard = runtime.clone();
            if let Some(interval) = runtime.reload_interval {
                if let Ok(mut next) = self.next_reload.lock() {
                    *next = Some(Instant::now() + interval);
                }
            }
        }
        let config = load_server_config_async(&runtime).await?;
        self.config.store(Arc::new(config));
        self.restart_watcher(runtime)?;
        Ok(())
    }
}

impl Drop for HttpTlsProvider {
    fn drop(&mut self) {
        self.stop_watcher();
    }
}

fn load_server_config_sync(runtime: &HttpTlsRuntime) -> Result<RustlsServerConfig> {
    let cert_bytes = std::fs::read(&runtime.cert_path).with_context(|| {
        format!(
            "konnte Zertifikat '{}' nicht lesen",
            runtime.cert_path.display()
        )
    })?;
    let key_bytes = std::fs::read(&runtime.key_path).with_context(|| {
        format!(
            "konnte Schlüssel '{}' nicht lesen",
            runtime.key_path.display()
        )
    })?;
    build_server_config(runtime, cert_bytes, key_bytes)
}

async fn load_server_config_async(runtime: &HttpTlsRuntime) -> Result<RustlsServerConfig> {
    let cert_path = runtime.cert_path.clone();
    let key_path = runtime.key_path.clone();
    let cert_bytes = tokio::fs::read(&cert_path)
        .await
        .with_context(|| format!("konnte Zertifikat '{}' nicht lesen", cert_path.display()))?;
    let key_bytes = tokio::fs::read(&key_path)
        .await
        .with_context(|| format!("konnte Schlüssel '{}' nicht lesen", key_path.display()))?;
    build_server_config(runtime, cert_bytes, key_bytes)
}

fn build_server_config(
    runtime: &HttpTlsRuntime,
    cert_bytes: Vec<u8>,
    key_bytes: Vec<u8>,
) -> Result<RustlsServerConfig> {
    let mut cert_reader: &[u8] = &cert_bytes;
    let cert_chain = certs(&mut cert_reader)
        .map_err(|_| anyhow!("Zertifikat konnte nicht geparst werden"))?
        .into_iter()
        .map(rustls::Certificate)
        .collect::<Vec<_>>();
    if cert_chain.is_empty() {
        return Err(anyhow!("keine Zertifikatsketten gefunden"));
    }

    let mut key_reader: &[u8] = &key_bytes;
    let mut keys = pkcs8_private_keys(&mut key_reader)
        .map_err(|_| anyhow!("Privater Schlüssel (PKCS8) konnte nicht geparst werden"))?;
    if keys.is_empty() {
        key_reader = &key_bytes;
        keys = rsa_private_keys(&mut key_reader)
            .map_err(|_| anyhow!("Privater Schlüssel (RSA) konnte nicht geparst werden"))?;
    }
    let key = keys
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("keinen privaten Schlüssel gefunden"))?;
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
            other => Err(anyhow!(format!("unbekannte Cipher Suite: {other}"))),
        })
        .collect()
}

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="de">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Fenrir Control Plane</title>
    <style>
      :root {
        color-scheme: dark light;
        font-family: "Inter", system-ui, -apple-system, "Segoe UI", sans-serif;
        background: radial-gradient(circle at 15% 15%, #182b44 0%, #0a111d 65%, #05070c 100%);
        color: #e8f1ff;
      }
      body {
        margin: 0;
        min-height: 100vh;
        display: flex;
        justify-content: center;
        background-image:
          radial-gradient(1100px circle at 0% 0%, rgba(77, 137, 255, 0.18), transparent 55%),
          radial-gradient(900px circle at 95% 10%, rgba(52, 215, 200, 0.18), transparent 60%);
        padding: 4rem 2.5rem 5rem;
      }
      main.shell {
        width: min(1200px, 100%);
        display: grid;
        gap: 2.6rem;
      }
      header.topbar {
        display: grid;
        grid-template-columns: auto auto;
        align-items: center;
        gap: 1.5rem;
        padding: 1.2rem 1.6rem;
        border-radius: 1.6rem;
        background: rgba(11, 20, 33, 0.82);
        border: 1px solid rgba(120, 170, 244, 0.22);
        box-shadow: 0 28px 58px rgba(4, 10, 22, 0.34);
        backdrop-filter: blur(16px);
        height: 54px;
        margin-bottom: 30px;
      }
      header.topbar .brand {
        display: flex;
        align-items: center;
        gap: 1rem;
      }
      header.topbar .brand .logo {
        width: 54px;
        height: 54px;
        border-radius: 16px;
        background: linear-gradient(140deg, #4f8efd 0%, #33d5c4 100%);
        display: grid;
        place-items: center;
        color: #041021;
        font-weight: 700;
        letter-spacing: 0.08em;
      }
      header.topbar .brand .title {
        display: flex;
        flex-direction: column;
        gap: 0.25rem;
      }
      header.topbar .brand .title span:first-child {
        text-transform: uppercase;
        letter-spacing: 0.18em;
        font-size: 0.78rem;
        color: rgba(156, 190, 255, 0.72);
      }
      header.topbar .brand .title span:last-child {
        font-size: 1.35rem;
        font-weight: 600;
        color: #f6f9ff;
      }
      header.topbar .actions {
        justify-self: end;
        display: flex;
        gap: 0.8rem;
      }
      header.topbar .actions button {
        appearance: none;
        border: 0;
        border-radius: 999px;
        padding: 0.55rem 1.25rem;
        font-size: 0.82rem;
        letter-spacing: 0.15em;
        text-transform: uppercase;
        background: linear-gradient(135deg, rgba(78, 156, 255, 0.94), rgba(48, 215, 198, 0.94));
        color: #061326;
        cursor: pointer;
        font-weight: 700;
        transition: transform 0.2s ease, box-shadow 0.2s ease;
      }
      header.topbar .actions button:hover:not(:disabled) {
        transform: translateY(-1px);
        box-shadow: 0 10px 24px rgba(60, 160, 255, 0.35);
      }
      header.topbar .actions button:disabled {
        filter: grayscale(1) opacity(0.6);
        cursor: not-allowed;
      }
      nav.tabbar {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
        gap: 0.6rem;
        padding: 0.65rem 0.75rem;
        border-radius: 1.4rem;
        background: rgba(9, 18, 34, 0.78);
        border: 1px solid rgba(120, 170, 244, 0.22);
        box-shadow: inset 0 0 0 1px rgba(76, 128, 210, 0.12);
        backdrop-filter: blur(14px);
        width: 100%;
        max-width: 860px;
        margin: 0 auto;
        align-items: stretch;
        height: 46px;
      }
      nav.tabbar button {
        appearance: none;
        border: 0;
        border-radius: 999px;
        padding: 0.45rem 1.1rem;
        font-size: 0.78rem;
        letter-spacing: 0.12em;
        text-transform: uppercase;
        font-weight: 600;
        cursor: pointer;
        background: rgba(82, 134, 214, 0.24);
        color: rgba(220, 232, 255, 0.88);
        transition: background 0.2s ease, color 0.2s ease, transform 0.2s ease;
        display: flex;
        align-items: center;
        justify-content: center;
        min-height: 44px;
      }
      nav.tabbar button:hover {
        background: rgba(108, 160, 244, 0.32);
        transform: translateY(-1px);
      }
      nav.tabbar button[data-active="true"] {
        background: linear-gradient(135deg, rgba(78, 156, 255, 0.82), rgba(48, 215, 198, 0.82));
        color: #041021;
        box-shadow: 0 14px 26px rgba(56, 122, 214, 0.38);
      }
      section.page {
        display: none;
        gap: 1.8rem;
      }
      section.page[data-visible="true"] {
        display: grid;
      }
      section.hero {
        display: grid;
        gap: 1.6rem;
      }
      section.hero h2 {
        margin: 0;
        font-size: clamp(2.6rem, 5vw, 3.4rem);
        letter-spacing: -0.018em;
        color: #f4f8ff;
      }
      section.hero p {
        margin: 0;
        font-size: 1.08rem;
        color: #9db7da;
        max-width: 72ch;
        line-height: 1.6;
      }
      .alert {
        display: none;
        padding: 1rem 1.2rem;
        border-radius: 1.1rem;
        border: 1px solid rgba(124, 176, 248, 0.28);
        background: rgba(15, 26, 42, 0.78);
        font-size: 0.94rem;
        color: #9ec3ff;
        box-shadow: inset 0 0 0 1px rgba(126, 180, 250, 0.12);
      }
      .alert[data-visible="true"] {
        display: block;
      }
      .meta {
        display: flex;
        flex-wrap: wrap;
        gap: 0.75rem;
        font-size: 0.95rem;
        color: #87a8d5;
      }
      .meta span {
        padding: 0.55rem 1.05rem;
        border-radius: 999px;
        background: rgba(128, 172, 242, 0.16);
        border: 1px solid rgba(128, 178, 250, 0.28);
      }
      section.summary-grid {
        display: grid;
        gap: 1.4rem;
        grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      }
      section.management-grid {
        display: grid;
        gap: 1.4rem;
        grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
      }
      .card {
        padding: 1.7rem;
        border-radius: 1.4rem;
        background: rgba(16, 27, 44, 0.82);
        border: 1px solid rgba(122, 170, 240, 0.24);
        box-shadow: 0 32px 62px rgba(4, 10, 22, 0.38);
        backdrop-filter: blur(16px);
        display: grid;
        gap: 0.85rem;
      }
      .card h3 {
        margin: 0;
        font-size: 0.92rem;
        text-transform: uppercase;
        letter-spacing: 0.18em;
        color: #7da8e6;
      }
      .metric-value {
        font-size: 2.1rem;
        font-weight: 600;
        color: #f2f6ff;
      }
      .metric-hint {
        font-size: 0.92rem;
        color: #99b7da;
      }
      .token-card .token-input {
        display: flex;
        flex-wrap: wrap;
        gap: 0.75rem;
      }
      .token-card input {
        flex: 1 1 260px;
        padding: 0.7rem 1.1rem;
        border-radius: 1.05rem;
        border: 1px solid rgba(126, 176, 248, 0.32);
        background: rgba(13, 22, 36, 0.92);
        color: #f0f6ff;
        font-size: 0.95rem;
        letter-spacing: 0.05em;
      }
      .token-card input:focus {
        outline: none;
        border-color: rgba(92, 156, 255, 0.6);
        box-shadow: 0 0 0 3px rgba(92, 156, 255, 0.2);
      }
      .token-status {
        font-size: 0.86rem;
        color: #95bbf1;
      }
      .control-card .button-row {
        display: flex;
        flex-wrap: wrap;
        gap: 0.75rem;
      }
      .control-card button {
        appearance: none;
        border: 0;
        border-radius: 0.95rem;
        padding: 0.6rem 1.1rem;
        font-size: 0.82rem;
        letter-spacing: 0.12em;
        text-transform: uppercase;
        background: rgba(104, 155, 238, 0.32);
        color: #eef4ff;
        cursor: pointer;
        transition: background 0.2s ease, transform 0.2s ease;
      }
      .control-card button:hover:not(:disabled) {
        background: rgba(122, 173, 252, 0.46);
        transform: translateY(-1px);
      }
      .control-card button:disabled {
        background: rgba(94, 118, 160, 0.26);
        color: rgba(210, 226, 250, 0.6);
        cursor: not-allowed;
      }
      .telemetry-grid {
        display: grid;
        gap: 1.6rem;
        grid-template-columns: minmax(0, 2fr) minmax(0, 1fr);
      }
      .telemetry-stats {
        display: grid;
        gap: 1.2rem;
        grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      }
      .telemetry-stat-value {
        font-size: clamp(2.2rem, 3vw, 2.8rem);
        font-weight: 600;
        color: #f2f6ff;
      }
      .telemetry-stat-note {
        font-size: 0.94rem;
        color: rgba(156, 190, 255, 0.72);
        letter-spacing: 0.04em;
      }
      @media (max-width: 980px) {
        .telemetry-grid {
          grid-template-columns: minmax(0, 1fr);
        }
      }
      .chart-card {
        position: relative;
        overflow: hidden;
        display: grid;
        gap: 1.1rem;
      }
      .chart-card canvas.metric-chart {
        width: 100%;
        max-width: 100%;
        display: block;
        border-radius: 1rem;
        background: rgba(6, 14, 24, 0.65);
        border: 1px solid rgba(88, 134, 212, 0.26);
        box-shadow: inset 0 0 0 1px rgba(70, 118, 196, 0.14);
        aspect-ratio: auto;
        height: clamp(200px, 24vh, 260px);
      }
      .chart-legend {
        display: flex;
        flex-wrap: wrap;
        gap: 0.6rem;
        align-items: center;
      }
      .chart-legend .legend-item {
        display: inline-flex;
        align-items: center;
        gap: 0.4rem;
        padding: 0.35rem 0.75rem;
        border-radius: 0.9rem;
        background: rgba(28, 46, 76, 0.58);
        border: 1px solid rgba(102, 150, 226, 0.34);
        letter-spacing: 0.08em;
        text-transform: uppercase;
        font-size: 0.72rem;
        color: #dce6ff;
      }
      .chart-legend .legend-swatch {
        width: 10px;
        height: 10px;
        border-radius: 999px;
        box-shadow: 0 0 0 2px rgba(4, 10, 22, 0.38);
      }
      .chart-legend .legend-label {
        white-space: nowrap;
      }
      .chart-legend .legend-value {
        font-variant-numeric: tabular-nums;
        color: rgba(170, 196, 232, 0.85);
      }
      .chart-legend .legend-empty {
        font-size: 0.82rem;
        color: rgba(156, 190, 255, 0.72);
      }
      .chart-tooltip {
        position: absolute;
        top: 0;
        left: 0;
        display: none;
        min-width: 180px;
        padding: 0.75rem 0.9rem;
        border-radius: 0.9rem;
        background: rgba(14, 24, 38, 0.92);
        border: 1px solid rgba(120, 170, 244, 0.42);
        box-shadow: 0 18px 40px rgba(4, 10, 22, 0.45);
        backdrop-filter: blur(12px);
        pointer-events: none;
        z-index: 5;
        transform: translate(-9999px, -9999px);
      }
      .chart-tooltip[data-visible="true"] {
        display: flex;
        flex-direction: column;
        gap: 0.5rem;
      }
      .chart-tooltip .tooltip-header {
        font-size: 0.78rem;
        letter-spacing: 0.12em;
        text-transform: uppercase;
        color: rgba(176, 206, 255, 0.78);
      }
      .chart-tooltip .tooltip-body {
        display: grid;
        gap: 0.35rem;
      }
      .chart-tooltip .tooltip-row {
        display: grid;
        grid-template-columns: auto 1fr auto;
        align-items: center;
        gap: 0.55rem;
        font-size: 0.88rem;
        color: #eaf3ff;
      }
      .chart-tooltip .tooltip-swatch {
        width: 10px;
        height: 10px;
        border-radius: 999px;
        box-shadow: 0 0 0 2px rgba(4, 16, 32, 0.45);
      }
      .chart-tooltip .tooltip-label {
        white-space: nowrap;
      }
      .chart-tooltip .tooltip-value {
        font-variant-numeric: tabular-nums;
        color: rgba(196, 218, 255, 0.92);
      }
      .chart-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        flex-wrap: wrap;
        gap: 0.75rem;
      }
      .chart-header-left {
        display: flex;
        flex-direction: column;
        gap: 0.35rem;
      }
      .chart-header-left small {
        color: rgba(162, 190, 230, 0.78);
        font-weight: 500;
      }
      .chart-range {
        display: inline-flex;
        align-items: center;
        gap: 0.4rem;
        background: rgba(18, 34, 58, 0.6);
        border: 1px solid rgba(102, 150, 226, 0.28);
        border-radius: 999px;
        padding: 0.25rem 0.35rem;
        flex-wrap: wrap;
      }
      .chart-range button {
        appearance: none;
        border: 0;
        border-radius: 999px;
        padding: 0.3rem 0.85rem;
        font-size: 0.72rem;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        background: transparent;
        color: rgba(188, 210, 246, 0.85);
        cursor: pointer;
        transition: background 0.2s ease, color 0.2s ease;
      }
      .chart-range button[data-active="true"] {
        background: linear-gradient(135deg, rgba(78, 156, 255, 0.32), rgba(48, 215, 198, 0.32));
        color: #f3f8ff;
      }
      .chart-range button:hover {
        background: rgba(88, 138, 220, 0.28);
        color: #ffffff;
      }
      .chart-empty {
        display: none;
        border-radius: 1rem;
        padding: 1rem 1.2rem;
        background: rgba(12, 24, 42, 0.78);
        border: 1px dashed rgba(120, 168, 240, 0.32);
        color: rgba(170, 196, 232, 0.82);
        text-align: center;
      }
      .chart-empty[data-visible="true"] {
        display: block;
      }
      .metric-table-card {
        display: flex;
        flex-direction: column;
        gap: 0.85rem;
      }
      .metric-table-scroll {
        overflow-y: auto;
        max-height: clamp(160px, 24vh, 240px);
        border-radius: 1rem;
        border: 1px solid rgba(88, 134, 212, 0.16);
        background: rgba(12, 20, 34, 0.55);
        box-shadow: inset 0 0 0 1px rgba(68, 106, 184, 0.12);
      }
      .metric-table-card table {
        width: 100%;
        border-collapse: collapse;
      }
      .metric-table-card th,
      .metric-table-card td {
        padding: 0.6rem 0.4rem;
        border-bottom: 1px solid rgba(120, 168, 240, 0.18);
        font-size: 0.88rem;
      }
      .metric-table-card th {
        text-transform: uppercase;
        letter-spacing: 0.12em;
        color: rgba(156, 190, 255, 0.76);
        font-size: 0.75rem;
      }
      .metric-table-card td:first-child {
        color: #f0f6ff;
      }
      .metric-table-card td:last-child {
        text-align: right;
        color: rgba(198, 218, 248, 0.9);
        font-variant-numeric: tabular-nums;
      }
      section.services {
        display: grid;
        gap: 1.5rem;
      }
      .service-header {
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        gap: 1rem;
      }
      .service-header h2 {
        margin: 0;
        font-size: 1.48rem;
        color: #f3f7ff;
      }
      .service-header small {
        color: #88aad0;
      }
      .service-board {
        border-radius: 1.45rem;
        background: rgba(14, 24, 38, 0.88);
        border: 1px solid rgba(130, 178, 244, 0.24);
        box-shadow: 0 34px 64px rgba(5, 10, 22, 0.34);
        backdrop-filter: blur(16px);
        overflow: hidden;
      }
      table.service-table {
        width: 100%;
        border-collapse: collapse;
      }
      table.service-table thead {
        background: rgba(26, 46, 68, 0.66);
      }
      table.service-table th,
      table.service-table td {
        padding: 1rem 1.15rem;
        text-align: left;
        font-size: 0.95rem;
        vertical-align: middle;
      }
      table.service-table tbody tr {
        border-top: 1px solid rgba(130, 176, 242, 0.18);
      }
      .status-pill {
        display: inline-flex;
        align-items: center;
        gap: 0.45rem;
        font-weight: 600;
        letter-spacing: 0.05em;
        padding: 0.35rem 0.85rem;
        border-radius: 999px;
        text-transform: uppercase;
        font-size: 0.78rem;
        max-width: 18ch;
        justify-content: center;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }
      table.service-table td.service-id,
      table.service-table td.service-name,
      table.service-table td.note-cell {
        word-break: break-word;
        overflow-wrap: anywhere;
      }
      table.service-table td.status-cell {
        width: 1%;
      }
      .status-active { background: rgba(71, 209, 167, 0.18); color: #4ef0bc; }
      .status-starting { background: rgba(224, 176, 49, 0.2); color: #f5ce6e; }
      .status-degraded { background: rgba(214, 120, 20, 0.24); color: #ffb372; }
      .status-failed { background: rgba(207, 60, 82, 0.24); color: #ff8ba3; }
      .status-stopped { background: rgba(113, 137, 169, 0.24); color: #a9bfdc; }
      .tags-cell {
        text-align: center;
      }
      .tag-list {
        display: inline-flex;
        flex-wrap: wrap;
        justify-content: center;
        align-items: center;
        gap: 0.45rem;
        width: 100%;
      }
      .tag {
        padding: 0.28rem 0.65rem;
        border-radius: 999px;
        font-size: 0.78rem;
        background: rgba(134, 176, 244, 0.22);
        border: 1px solid rgba(134, 176, 244, 0.33);
        letter-spacing: 0.05em;
      }
      .tag-core { color: #ffd78f; border-color: rgba(255, 215, 143, 0.42); background: rgba(255, 215, 143, 0.18); }
      .tag-platform { color: #94caff; }
      .row-actions {
        display: inline-flex;
        flex-wrap: wrap;
        gap: 0.5rem;
      }
      .row-actions button {
        appearance: none;
        border: 0;
        border-radius: 0.85rem;
        padding: 0.45rem 0.9rem;
        font-size: 0.78rem;
        letter-spacing: 0.1em;
        text-transform: uppercase;
        background: rgba(105, 155, 232, 0.32);
        color: #eef4ff;
        cursor: pointer;
        transition: background 0.2s ease;
      }
      .row-actions button:hover:not(:disabled) {
        background: rgba(122, 173, 252, 0.46);
      }
      .row-actions button:disabled {
        background: rgba(94, 118, 160, 0.26);
        color: rgba(210, 226, 250, 0.6);
        cursor: not-allowed;
      }
      section.audit {
        display: grid;
        gap: 1rem;
      }
      .audit-board {
        border-radius: 1.2rem;
        background: rgba(10, 18, 32, 0.92);
        border: 1px solid rgba(100, 140, 210, 0.24);
        box-shadow: 0 24px 48px rgba(5, 10, 22, 0.28);
        overflow: hidden;
      }
      table.audit-table {
        width: 100%;
        border-collapse: collapse;
        font-size: 0.85rem;
      }
      table.audit-table thead {
        background: rgba(20, 34, 52, 0.72);
        text-transform: uppercase;
        letter-spacing: 0.08em;
        font-size: 0.75rem;
      }
      table.audit-table th,
      table.audit-table td {
        padding: 0.75rem 1rem;
        text-align: left;
        vertical-align: top;
      }
      table.audit-table tbody tr {
        border-top: 1px solid rgba(100, 140, 210, 0.16);
      }
      .audit-meta {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 0.85rem 1.1rem;
        background: rgba(15, 28, 48, 0.84);
        border-bottom: 1px solid rgba(100, 140, 210, 0.24);
        gap: 0.75rem;
        flex-wrap: wrap;
      }
      .audit-meta h2 {
        margin: 0;
        font-size: 1.15rem;
        color: #eff5ff;
      }
      .audit-meta small {
        color: rgba(162, 190, 230, 0.8);
        font-weight: 500;
      }
      .audit-meta button[data-audit-refresh] {
        appearance: none;
        border: 0;
        border-radius: 0.75rem;
        padding: 0.45rem 0.9rem;
        font-size: 0.72rem;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        background: rgba(88, 134, 212, 0.28);
        color: #e9f2ff;
        cursor: pointer;
        transition: background 0.2s ease;
      }
      .audit-meta button[data-audit-refresh]:hover:not(:disabled) {
        background: rgba(112, 156, 236, 0.4);
      }
      .pill {
        display: inline-flex;
        align-items: center;
        padding: 0.18rem 0.55rem;
        border-radius: 999px;
        font-size: 0.7rem;
        letter-spacing: 0.05em;
      }
      .pill.success {
        background: rgba(60, 182, 140, 0.18);
        color: #42e0ad;
      }
      .pill.failure {
        background: rgba(197, 70, 92, 0.2);
        color: #ff8fa5;
      }
      .pill.denied {
        background: rgba(210, 142, 64, 0.2);
        color: #ffce85;
      }
      .metadata-chips {
        display: flex;
        flex-wrap: wrap;
        gap: 0.35rem;
      }
      .metadata-chip {
        padding: 0.18rem 0.45rem;
        border-radius: 0.6rem;
        font-size: 0.68rem;
        border: 1px solid rgba(120, 162, 230, 0.24);
        background: rgba(120, 162, 230, 0.12);
      }
      .metadata-chip[data-redacted="true"] {
        opacity: 0.66;
        font-style: italic;
      }
      footer.note {
        text-align: center;
        font-size: 0.84rem;
        color: rgba(154, 182, 218, 0.78);
      }
      @media (max-width: 760px) {
        body { padding: 3.2rem 1.6rem 4rem; }
        header.topbar { grid-template-columns: 1fr; }
        header.topbar .actions { justify-content: flex-end; }
        nav.tabbar { grid-template-columns: 1fr; max-width: 100%; }
        nav.tabbar button { width: 100%; }
        table.service-table th:nth-child(4),
        table.service-table td:nth-child(4),
        table.service-table th:nth-child(5),
        table.service-table td:nth-child(5),
        table.service-table th:nth-child(6),
        table.service-table td:nth-child(6) { display: none; }
        .token-card .token-input { flex-direction: column; }
      }
    </style>
  </head>
  <body>
    <main class="shell">
    <div>
      <header class="topbar">
        <div class="brand">
          <div class="logo">FN</div>
          <div class="title">
            <span>operations</span>
            <span>Fenrir Control Plane</span>
          </div>
        </div>
        <div class="actions">
          <button type="button" data-test-btn disabled>Token Test</button>
        </div>
      </header>

      <nav class="tabbar" role="tablist" aria-label="Bereichsauswahl">
        <button type="button" role="tab" data-page-trigger="overview" data-active="true" aria-selected="true">Übersicht</button>
        <button type="button" role="tab" data-page-trigger="services" aria-selected="false">Services</button>
        <button type="button" role="tab" data-page-trigger="telemetry" aria-selected="false">Telemetry</button>
        <button type="button" role="tab" data-page-trigger="audit" aria-selected="false">Audit</button>
      </nav>
       </div>
      <section class="page" data-page="overview" data-visible="true">
        <section class="hero">
          <h2>Überblick &amp; Steuerung für Fenrir</h2>
          <p>
            Echtzeitstatus, geschützte Service-Aktionen und ein aufgeräumtes Interface ermöglichen einen ruhigen Betrieb.
            Jeder Eingriff läuft über RBAC und Audit-Logs.
          </p>
        </section>
        <div class="alert" data-alert></div>
        <div class="meta" data-meta>
          <span>lade Applikationsdaten …</span>
        </div>
        <section class="summary-grid">
          <article class="card metric-card" data-card="app">
            <h3>Applikation</h3>
            <div class="metric-value" data-app-name>–</div>
            <div class="metric-hint" data-app-version>Version wird geladen …</div>
          </article>
          <article class="card metric-card" data-card="uptime">
            <h3>Uptime</h3>
            <div class="metric-value" data-uptime>–</div>
            <div class="metric-hint">Quelle: Telemetrie</div>
          </article>
          <article class="card metric-card" data-card="health">
            <h3>Status</h3>
            <div class="metric-value" data-health>—</div>
            <div class="metric-hint" data-health-note>Prüfe Readiness-Status …</div>
          </article>
        </section>
        <section class="management-grid">
          <article class="card token-card">
            <h3>API Token</h3>
            <div class="token-input">
              <input
                type="password"
                autocomplete="off"
                placeholder="Bearer Token hier einfügen"
                aria-label="Control Plane Token"
                data-token-input
              />
            </div>
            <div class="token-status" data-token-status>
              Token nicht gesetzt – Anfragen erfolgen ohne Auth.
            </div>
          </article>
        </section>
      </section>

      <section class="page" data-page="telemetry" data-visible="false">
        <div class="telemetry-stats">
          <article class="card metric-card">
            <h3>CPU Auslastung</h3>
            <div class="telemetry-stat-value" data-metric-cpu>–</div>
            <div class="telemetry-stat-note" data-metric-cpu-note>Keine Daten verfügbar.</div>
          </article>
          <article class="card metric-card">
            <h3>Speichernutzung</h3>
            <div class="telemetry-stat-value" data-metric-mem>–</div>
            <div class="telemetry-stat-note" data-metric-mem-note>Keine Daten verfügbar.</div>
          </article>
          <article class="card metric-card">
            <h3>Disk I/O</h3>
            <div class="telemetry-stat-value" data-metric-io>–</div>
            <div class="telemetry-stat-note" data-metric-io-note>Keine Daten verfügbar.</div>
          </article>
        </div>
        <div class="telemetry-grid">
          <article class="card chart-card">
            <div class="chart-header">
              <div class="chart-header-left">
                <h3>Performance Verlauf</h3>
                <small data-telemetry-note>Telemetrie wird geladen …</small>
              </div>
              <div class="chart-range" role="tablist" aria-label="Zeitraum">
                <button type="button" data-history-range="1h" data-active="true">1h</button>
                <button type="button" data-history-range="3h">3h</button>
                <button type="button" data-history-range="24h">24h</button>
              </div>
            </div>
            <canvas class="metric-chart" data-metric-chart width="960" height="320"></canvas>
            <div class="chart-legend" data-metric-legend>
              <span class="legend-empty">Es liegen noch keine Messwerte vor.</span>
            </div>
            <div class="chart-tooltip" data-chart-tooltip>
              <div class="tooltip-header" data-tooltip-time>–</div>
              <div class="tooltip-body" data-tooltip-body></div>
            </div>
            <div class="chart-empty" data-chart-empty data-visible="true">
              Noch keine Telemetriedaten verfügbar.
            </div>
          </article>
          <article class="card metric-table-card">
            <h3>Aktuelle Kennzahlen</h3>
            <div class="metric-table-scroll">
              <table class="metric-table">
                <thead>
                  <tr>
                    <th>Metrik</th>
                    <th>Wert</th>
                  </tr>
                </thead>
                <tbody data-metric-list>
                  <tr>
                    <td colspan="2">Noch keine Daten geladen.</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </article>
        </div>
      </section>

      <section class="page" data-page="services" data-visible="false">
        <section class="management-grid service-actions-grid">
          <article class="card control-card">
            <h3>Service Aktionen</h3>
            <div class="button-row">
              <button type="button" data-bulk-start>Start All</button>
              <button type="button" data-bulk-stop>Stop All</button>
              <button type="button" data-bulk-restart>Restart All</button>
            </div>
            <div class="metric-hint">Nur nicht-core Services werden beeinflusst.</div>
          </article>
        </section>

        <section class="services">
          <div class="service-header">
            <h2>Registrierte Services</h2>
            <small data-service-meta>lade Registry …</small>
          </div>
          <div class="service-board">
            <table class="service-table">
              <thead>
                <tr>
                  <th>ID</th>
                  <th>Name</th>
                  <th>Status</th>
                  <th>Tags</th>
                  <th>Hinweis</th>
                  <th>Aktionen</th>
                </tr>
              </thead>
              <tbody data-services>
                <tr>
                  <td colspan="6">Service-Registry wird abgefragt …</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>
    </section>

    <section class="page" data-page="audit" data-visible="false">
      <section class="audit">
        <div class="audit-board">
          <div class="audit-meta">
            <div>
              <h2>Audit Log</h2>
              <small data-audit-meta>–</small>
            </div>
            <div class="chart-range audit-range" role="tablist" aria-label="Audit-Zeitraum">
              <button type="button" data-audit-range="1h" data-active="true">1h</button>
              <button type="button" data-audit-range="3h">3h</button>
              <button type="button" data-audit-range="24h">24h</button>
            </div>
            <button type="button" data-audit-refresh>Refresh</button>
          </div>
          <table class="audit-table">
            <thead>
              <tr>
                <th>Zeitpunkt</th>
                <th>Aktion</th>
                <th>Ziel</th>
                <th>Ergebnis</th>
                <th>Akteur</th>
                <th>Details</th>
              </tr>
            </thead>
            <tbody data-audit-events>
              <tr>
                <td colspan="6">Keine Audit-Ereignisse geladen.</td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>
    </section>

      <footer class="note">
        Zugriff auf erweiterte Aktionen erfolgt über die CLI oder autorisierte API-Clients. Alle
        Eingriffe werden auditierbar festgehalten.
      </footer>
    </main>

    <script>
      
      const REFRESH_INTERVAL_MS = 15000;
      const UPTIME_TICK_MS = 1000;
      const CHART_METRIC_KEYS = [
        'process.cpu.usage_percent',
        'process.memory.resident_bytes',
        'process.io.read_bytes_per_sec',
        'process.io.write_bytes_per_sec',
      ];
      const PINNED_METRIC_KEYS = [
        'process.cpu.usage_percent',
        'process.memory.resident_bytes',
        'process.memory.virtual_bytes',
        'process.io.read_bytes_per_sec',
        'process.io.write_bytes_per_sec',
      ];
      const METRIC_LABEL_OVERRIDES = {
        'process.cpu.usage_percent': 'CPU-Auslastung (Prozess)',
        'process.memory.resident_bytes': 'Arbeitsspeicher RSS (Prozess)',
        'process.memory.virtual_bytes': 'Arbeitsspeicher virtuell (Prozess)',
        'process.io.read_bytes_per_sec': 'I/O Lesen pro Sekunde',
        'process.io.write_bytes_per_sec': 'I/O Schreiben pro Sekunde',
      };
      const HISTORY_RANGE_DEFAULT = '1h';
      const HISTORY_RANGES = ['1h', '3h', '24h'];
      const HISTORY_RETENTION_MS = 24 * 60 * 60 * 1000;
      const AUDIT_HISTORY_RANGE_DEFAULT = '1h';
      const AUDIT_HISTORY_RANGES = ['1h', '3h', '24h'];
      const AUDIT_HISTORY_LIMIT = 1000;

      const metaEl = document.querySelector('[data-meta]');
      const alertEl = document.querySelector('[data-alert]');
      const svcBody = document.querySelector('[data-services]');
      const metaServices = document.querySelector('[data-service-meta]');
      const appName = document.querySelector('[data-app-name]');
      const appVersion = document.querySelector('[data-app-version]');
      const uptimeEl = document.querySelector('[data-uptime]');
      const healthValue = document.querySelector('[data-health]');
      const healthNote = document.querySelector('[data-health-note]');
      const tokenInput = document.querySelector('[data-token-input]');
      const tokenStatus = document.querySelector('[data-token-status]');
      const testButton = document.querySelector('[data-test-btn]');
      const bulkStartBtn = document.querySelector('[data-bulk-start]');
      const bulkStopBtn = document.querySelector('[data-bulk-stop]');
      const bulkRestartBtn = document.querySelector('[data-bulk-restart]');
      const auditBody = document.querySelector('[data-audit-events]');
      const auditMeta = document.querySelector('[data-audit-meta]');
      const auditRefreshBtn = document.querySelector('[data-audit-refresh]');
      const telemetryNote = document.querySelector('[data-telemetry-note]');
      const metricChartCanvas = document.querySelector('[data-metric-chart]');
      const metricPlaceholder = document.querySelector('[data-chart-empty]');
      const metricLegend = document.querySelector('[data-metric-legend]');
      const metricListBody = document.querySelector('[data-metric-list]');
      const metricCpuValue = document.querySelector('[data-metric-cpu]');
      const metricCpuNote = document.querySelector('[data-metric-cpu-note]');
      const metricMemValue = document.querySelector('[data-metric-mem]');
      const metricMemNote = document.querySelector('[data-metric-mem-note]');
      const metricIoValue = document.querySelector('[data-metric-io]');
      const metricIoNote = document.querySelector('[data-metric-io-note]');
      const chartTooltip = document.querySelector('[data-chart-tooltip]');
      const chartTooltipTime = document.querySelector('[data-tooltip-time]');
      const chartTooltipBody = document.querySelector('[data-tooltip-body]');
      const pageButtons = Array.from(document.querySelectorAll('[data-page-trigger]'));
      const pageContainers = new Map(
        Array.from(document.querySelectorAll('[data-page]')).map((el) => [el.dataset.page, el]),
      );
      const historyRangeButtons = Array.from(document.querySelectorAll('[data-history-range]'));
      const auditRangeButtons = Array.from(document.querySelectorAll('[data-audit-range]'));

      const TOKEN_KEY = 'fenrir-control-plane-token';
      const PAGE_STORAGE_KEY = 'fenrir-control-plane-page';

      let refreshHandle = null;
      let uptimeHandle = null;
      let uptimeBaseSeconds = null;
      let uptimeAnchor = null;
      let isLoading = false;
      let auditCache = [];
      let servicesCache = new Map();
      let eventSource = null;
      let sseWarned = false;
      const metricHistory = [];
      const METRIC_HISTORY_LIMIT = 20000;
      const METRIC_HISTORY_CHART_LIMIT = 2000;
      const METRIC_SERIES_MAX = 4;
      const METRIC_COLORS = ['#4f8efd', '#33d5c4', '#f4d35e', '#ed6a5a', '#c792ea', '#6ad1ff'];
      const numberFormatter = new Intl.NumberFormat('de-DE');
      let metricCtx = null;
      const SERVICE_STATUS_KEYS = ['starting', 'active', 'degraded', 'failed', 'standby', 'stopped'];
      const SERVICE_STATUS_FALLBACK = 'services.status.other';
      const SERVICE_TAG_KEYS = ['core', 'platform', 'auxiliary'];
      let chartHoverState = null;
      let currentHistoryRange = HISTORY_RANGE_DEFAULT;
      let historyLoaded = false;
      let historyLoading = false;
      let currentAuditRange = AUDIT_HISTORY_RANGE_DEFAULT;
      let auditHistoryLoaded = false;
      let auditHistoryLoading = false;

      const loadToken = () => {
        try {
          return localStorage.getItem(TOKEN_KEY) ?? '';
        } catch (_) {
          return '';
        }
      };

      const saveToken = (value) => {
        try {
          if (!value) {
            localStorage.removeItem(TOKEN_KEY);
          } else {
            localStorage.setItem(TOKEN_KEY, value);
          }
        } catch (_) {}
      };

      const rangeToMillis = (range) => {
        switch (range) {
          case '3h':
            return 3 * 60 * 60 * 1000;
          case '24h':
          case '1d':
            return 24 * 60 * 60 * 1000;
          case '1h':
          default:
            return 60 * 60 * 1000;
        }
      };

      const currentToken = () => tokenInput.value.trim();

      const setHistoryButtonsActive = (range) => {
        historyRangeButtons.forEach((button) => {
          const active = button.dataset.historyRange === range;
          button.dataset.active = active ? 'true' : 'false';
        });
      };

      const setAuditButtonsActive = (range) => {
        auditRangeButtons.forEach((button) => {
          const active = button.dataset.auditRange === range;
          button.dataset.active = active ? 'true' : 'false';
        });
      };

      const pruneMetricHistory = () => {
        const cutoff = Date.now() - HISTORY_RETENTION_MS;
        while (metricHistory.length > 0 && metricHistory[0].timestamp < cutoff) {
          metricHistory.shift();
        }
        if (metricHistory.length > METRIC_HISTORY_LIMIT) {
          metricHistory.splice(0, metricHistory.length - METRIC_HISTORY_LIMIT);
        }
      };

      const getHistoryForChart = () => {
        if (metricHistory.length <= METRIC_HISTORY_CHART_LIMIT) {
          return metricHistory;
        }
        const step = Math.ceil(metricHistory.length / METRIC_HISTORY_CHART_LIMIT);
        const sampled = [];
        for (let idx = 0; idx < metricHistory.length; idx += step) {
          sampled.push(metricHistory[idx]);
        }
        const last = metricHistory[metricHistory.length - 1];
        if (sampled[sampled.length - 1] !== last) {
          sampled.push(last);
        }
        return sampled;
      };

      const applyHistorySamples = (samples) => {
        metricHistory.length = 0;
        if (Array.isArray(samples) && samples.length > 0) {
          samples
            .slice()
            .sort((a, b) => a.timestamp_ms - b.timestamp_ms)
            .forEach((sample) => {
              metricHistory.push({
                timestamp: sample.timestamp_ms,
                counters: sample.metrics ?? {},
              });
            });
          pruneMetricHistory();
          if (metricHistory.length > 0) {
            const latestCounters = metricHistory[metricHistory.length - 1].counters;
            renderMetricList(latestCounters);
            updateTelemetrySummary(latestCounters);
          }
        }
        renderMetricChart();
      };

      const fetchTelemetryHistory = async (range, { background = false } = {}) => {
        if (historyLoading) {
          return;
        }
        historyLoading = true;
        try {
          const response = await fetchWithToken(`/metrics/history?range=${encodeURIComponent(range)}`);
          if (!response.ok) {
            if (!background && telemetryNote) {
              telemetryNote.textContent = `Historie nicht verfügbar (${response.status}).`;
            }
            return;
          }
          const body = await response.json();
          applyHistorySamples(body.samples ?? []);
          if (telemetryNote) {
            const stamp = new Date().toLocaleTimeString('de-DE');
            telemetryNote.textContent = `Stand: ${stamp} · Range ${body.range ?? range}`;
          }
          historyLoaded = true;
        } catch (error) {
          if (!background && telemetryNote) {
            telemetryNote.textContent = 'Telemetrie-Historie nicht verfügbar.';
          }
        } finally {
          historyLoading = false;
        }
      };

      const trimAuditCache = () => {
        if (!Array.isArray(auditCache)) {
          auditCache = [];
          return;
        }
        const rangeMs = rangeToMillis(currentAuditRange);
        const cutoff = Date.now() - rangeMs;
        auditCache = auditCache.filter((entry) => {
          const ts = Date.parse(entry.timestamp ?? '');
          return !Number.isFinite(ts) || ts >= cutoff;
        });
        if (auditCache.length > AUDIT_HISTORY_LIMIT) {
          auditCache.length = AUDIT_HISTORY_LIMIT;
        }
      };

      const applyAuditEvents = (events) => {
        if (!Array.isArray(events)) {
          auditCache = [];
        } else {
          auditCache = events;
        }
        trimAuditCache();
        renderAuditCache();
      };

      const fetchAuditHistory = async (range, { background = false } = {}) => {
        if (auditHistoryLoading) {
          return;
        }
        auditHistoryLoading = true;
        try {
          const response = await fetchWithToken(`/audit/history?range=${encodeURIComponent(range)}&limit=${AUDIT_HISTORY_LIMIT}`);
          if (!response.ok) {
            if (!background) {
              showAlert(`Audit-Historie nicht verfügbar (${response.status}).`);
            }
            return;
          }
          const body = await response.json();
          applyAuditEvents(body.events ?? []);
          auditMeta.textContent = `${auditCache.length} Einträge · Range ${body.range ?? range}`;
          auditHistoryLoaded = true;
        } catch (error) {
          if (!background) {
            showAlert('Audit-Historie nicht verfügbar.');
          }
        } finally {
          auditHistoryLoading = false;
        }
      };

      const authHeaders = () => {
        const token = currentToken();
        if (!token) {
          return {};
        }
        return {
          Authorization: token.startsWith('Bearer ') ? token : `Bearer ${token}`,
        };
      };

      const setTokenUi = (token) => {
        if (!token) {
          tokenStatus.textContent = 'Token nicht gesetzt – Anfragen erfolgen ohne Auth.';
          testButton.disabled = true;
        } else {
          tokenStatus.textContent = 'Token aktiv – geschützte Endpunkte verwenden nun Autorisierung.';
          testButton.disabled = false;
        }
      };

      const showAlert = (message) => {
        if (!message) {
          alertEl.dataset.visible = 'false';
          alertEl.textContent = '';
          return;
        }
        alertEl.dataset.visible = 'true';
        alertEl.textContent = message;
      };

      const formatDuration = (seconds) => {
        if (seconds == null) return '–';
        const days = Math.floor(seconds / 86400);
        const hours = Math.floor((seconds % 86400) / 3600);
        const minutes = Math.floor((seconds % 3600) / 60);
        if (days > 0) {
          return `${days}d ${hours}h`;
        }
        if (hours > 0) {
          return `${hours}h ${minutes}m`;
        }
        return `${minutes}m`;
      };

      const formatTimestamp = (value) => {
        if (!value) return '–';
        const date = new Date(value);
        if (Number.isNaN(date.getTime())) {
          return value;
        }
        return date.toLocaleString('de-DE', {
          year: 'numeric',
          month: '2-digit',
          day: '2-digit',
          hour: '2-digit',
          minute: '2-digit',
          second: '2-digit',
          hour12: false,
        });
      };

      const escapeHtml = (value) => {
        return String(value ?? '')
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;')
          .replace(/"/g, '&quot;')
          .replace(/'/g, '&#39;');
      };

      const formatNumber = (value) => {
        if (!Number.isFinite(value)) {
          return '–';
        }
        return numberFormatter.format(value);
      };

      const withAlpha = (hex, alpha) => {
        if (typeof hex !== 'string') {
          return hex;
        }
        const normalized = hex.startsWith('#') ? hex.slice(1) : hex;
        if (normalized.length !== 6) {
          return hex;
        }
        const r = parseInt(normalized.slice(0, 2), 16);
        const g = parseInt(normalized.slice(2, 4), 16);
        const b = parseInt(normalized.slice(4, 6), 16);
        const clampedAlpha = Math.min(Math.max(alpha ?? 1, 0), 1);
        if (!Number.isFinite(r) || !Number.isFinite(g) || !Number.isFinite(b)) {
          return hex;
        }
        return `rgba(${r}, ${g}, ${b}, ${clampedAlpha})`;
      };

      const formatChartTime = (timestamp) => {
        if (!Number.isFinite(timestamp)) {
          return '';
        }
        const date = new Date(timestamp);
        return date.toLocaleTimeString('de-DE', {
          hour: '2-digit',
          minute: '2-digit',
          second: '2-digit',
        });
      };

      const formatMetricLabel = (key) => {
        if (!key) {
          return 'metric';
        }
        return String(key)
          .replace(/^(services|process)\./, '')
          .replace(/\./g, ' › ')
          .replace(/_/g, ' ');
      };

      const formatPercent = (value, fractionDigits = 0) => {
        if (!Number.isFinite(value)) {
          return '–';
        }
        return `${value.toFixed(fractionDigits)} %`;
      };

      const formatBytes = (value) => {
        if (!Number.isFinite(value)) {
          return '–';
        }
        const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
        let amount = value;
        let unitIndex = 0;
        while (amount >= 1024 && unitIndex < units.length - 1) {
          amount /= 1024;
          unitIndex += 1;
        }
        const precision = amount >= 10 || unitIndex === 0 ? 0 : 1;
        return `${amount.toFixed(precision)} ${units[unitIndex]}`;
      };

      const formatThroughput = (value) => {
        if (!Number.isFinite(value)) {
          return '0 B/s';
        }
        if (value === 0) {
          return '0 B/s';
        }
        return `${formatBytes(value)}/s`;
      };

      const formatTooltipValue = (key, value) => {
        if (!Number.isFinite(value)) {
          return '–';
        }
        if (key.includes('.cpu.') || key.endsWith('.percent')) {
          return formatPercent(value, value < 10 ? 1 : 0);
        }
        if (key.includes('.memory.') || key.includes('.bytes_total')) {
          return formatBytes(value);
        }
        if (key.includes('.io.') && key.includes('_per_sec')) {
          return formatThroughput(value);
        }
        return formatNumber(value);
      };

      const hideChartTooltip = () => {
        if (!chartTooltip) {
          return;
        }
        chartTooltip.dataset.visible = 'false';
        chartTooltip.style.transform = 'translate(-9999px, -9999px)';
      };

      const filterChartCounters = (counters) => {
        if (!counters) {
          return {};
        }
        const preferred = CHART_METRIC_KEYS.filter((key) => key in counters);
        if (preferred.length === 0) {
          return {};
        }
        const mapped = {};
        preferred.forEach((key) => {
          mapped[key] = counters[key];
        });
        return mapped;
      };

      const resetTelemetrySummary = () => {
        if (metricCpuValue) {
          metricCpuValue.textContent = '–';
        }
        if (metricCpuNote) {
          metricCpuNote.textContent = 'Keine Prozessdaten verfügbar.';
        }
        if (metricMemValue) {
          metricMemValue.textContent = '–';
        }
        if (metricMemNote) {
          metricMemNote.textContent = 'Keine Prozessdaten verfügbar.';
        }
        if (metricIoValue) {
          metricIoValue.textContent = '–';
        }
        if (metricIoNote) {
          metricIoNote.textContent = 'Keine Prozessdaten verfügbar.';
        }
      };

      const updateTelemetrySummary = (counters) => {
        if (!counters || !Object.keys(counters).some((key) => key.startsWith('process.'))) {
          return;
        }

        if (metricCpuValue) {
          const cpuPercent = Number(counters['process.cpu.usage_percent']);
          if (Number.isFinite(cpuPercent)) {
            metricCpuValue.textContent = formatPercent(cpuPercent, cpuPercent < 10 ? 1 : 0);
            if (metricCpuNote) {
              if (cpuPercent >= 90) {
                metricCpuNote.textContent = 'Warnung: sehr hohe Auslastung.';
              } else if (cpuPercent >= 70) {
                metricCpuNote.textContent = 'Hinweis: erhöhte CPU-Last.';
              } else {
                metricCpuNote.textContent = 'CPU-Last unkritisch.';
              }
            }
          }
        }

        if (metricMemValue) {
          const residentBytes = Number(counters['process.memory.resident_bytes']);
          const virtualBytes = Number(counters['process.memory.virtual_bytes']);
          if (Number.isFinite(residentBytes)) {
            metricMemValue.textContent = formatBytes(residentBytes);
            if (metricMemNote) {
              if (Number.isFinite(virtualBytes) && virtualBytes > 0) {
                metricMemNote.textContent = `Virtuell: ${formatBytes(virtualBytes)}`;
              } else {
                metricMemNote.textContent = 'Residenter Speicher (RSS).';
              }
            }
          }
        }

        if (metricIoValue) {
          const readRate = Number(counters['process.io.read_bytes_per_sec']);
          const writeRate = Number(counters['process.io.write_bytes_per_sec']);
          const combinedRate = [readRate, writeRate]
            .filter((value) => Number.isFinite(value))
            .reduce((sum, value) => sum + value, 0);
          if (Number.isFinite(combinedRate)) {
            metricIoValue.textContent = formatThroughput(combinedRate);
            if (metricIoNote) {
              const readText = Number.isFinite(readRate) ? formatThroughput(readRate) : '0 B/s';
              const writeText = Number.isFinite(writeRate) ? formatThroughput(writeRate) : '0 B/s';
              metricIoNote.textContent = `Lesen ${readText} · Schreiben ${writeText}`;
            }
          }
        }
      };

      const renderMetricLegend = (entries) => {
        if (!metricLegend) {
          return;
        }
        const items = Array.isArray(entries) ? entries : [];
        if (items.length === 0) {
          metricLegend.innerHTML = '<span class="legend-empty">Es liegen noch keine Messwerte vor.</span>';
          return;
        }
        metricLegend.innerHTML = items
          .map((entry) => {
            const baseLabel = entry.label ?? METRIC_LABEL_OVERRIDES[entry.key] ?? formatMetricLabel(entry.key);
            const label = escapeHtml(baseLabel);
            const valueText = escapeHtml(formatTooltipValue(entry.key, entry.value));
            const color = entry.color || '#4f8efd';
            return `
              <span class="legend-item">
                <span class="legend-swatch" style="background:${color}"></span>
                <span class="legend-label">${label}</span>
                <span class="legend-value">${valueText}</span>
              </span>
            `;
          })
          .join('');
      };

      const setChartEmpty = (empty, message) => {
        if (!metricChartCanvas || !metricPlaceholder) {
          return;
        }
        if (empty) {
          metricChartCanvas.style.display = 'none';
          metricPlaceholder.dataset.visible = 'true';
          if (message) {
            metricPlaceholder.textContent = message;
          }
          renderMetricLegend([]);
          hideChartTooltip();
        } else {
          metricChartCanvas.style.display = 'block';
          metricPlaceholder.dataset.visible = 'false';
          if (message) {
            metricPlaceholder.textContent = message;
          }
        }
      };

      const renderMetricList = (counters) => {
        if (!metricListBody) {
          return;
        }
        const entries = Object.entries(counters || {}).filter(([, value]) => Number.isFinite(value));
        if (entries.length === 0) {
          metricListBody.innerHTML = '<tr><td colspan="2">Keine Telemetriedaten gemeldet.</td></tr>';
          return;
        }

        const pinned = PINNED_METRIC_KEYS
          .map((key) => [key, Number(counters?.[key])])
          .filter(([, value]) => Number.isFinite(value));
        const seen = new Set(pinned.map(([key]) => key));

        const dynamic = entries
          .filter(([key]) => !seen.has(key))
          .sort((a, b) => b[1] - a[1]);

        const combined = [...pinned, ...dynamic].slice(0, 20);
        metricListBody.innerHTML = combined
          .map(([name, value]) => {
            const label = escapeHtml(METRIC_LABEL_OVERRIDES[name] ?? formatMetricLabel(name));
            const valueText = escapeHtml(formatTooltipValue(name, value));
            return `<tr><td>${label}</td><td>${valueText}</td></tr>`;
          })
          .join('');
      };

      const renderMetricChart = () => {
        if (!metricChartCanvas) {
          return;
        }
        if (!metricCtx) {
          metricCtx = metricChartCanvas.getContext('2d');
        }
        if (!metricCtx) {
          return;
        }
        chartHoverState = null;
        const history = getHistoryForChart();
        if (history.length === 0) {
          metricCtx.clearRect(0, 0, metricChartCanvas.width, metricChartCanvas.height);
          renderMetricLegend([]);
          const placeholderText = metricPlaceholder ? metricPlaceholder.textContent : '';
          setChartEmpty(true, placeholderText || 'Keine Telemetriedaten gemeldet.');
          return;
        }

        setChartEmpty(false);
        const ctx = metricCtx;
        const width = metricChartCanvas.width;
        const height = metricChartCanvas.height;
        ctx.clearRect(0, 0, width, height);

        const padding = 48;
        const plotWidth = width - padding * 2;
        const plotHeight = height - padding * 2;

        const keys = new Set();
        history.forEach((sample) => {
          Object.keys(sample.counters).forEach((key) => keys.add(key));
        });
        if (keys.size === 0) {
          renderMetricLegend([]);
          setChartEmpty(true, 'Keine Telemetriedaten gemeldet.');
          return;
        }

        const latest = metricHistory[metricHistory.length - 1]?.counters ?? {};
        const prioritizedSeries = [];
        CHART_METRIC_KEYS.forEach((key) => {
          if (keys.has(key) && !prioritizedSeries.includes(key)) {
            prioritizedSeries.push(key);
          }
        });
        const otherSeries = Array.from(keys).filter((key) => !prioritizedSeries.includes(key));
        otherSeries.sort((a, b) => (latest[b] ?? 0) - (latest[a] ?? 0));
        const series = [...prioritizedSeries, ...otherSeries].slice(0, METRIC_SERIES_MAX);

        if (series.length === 0) {
          renderMetricLegend([]);
          setChartEmpty(true, 'Keine Telemetriedaten gemeldet.');
          return;
        }

        let minValue = Infinity;
        let maxValue = -Infinity;
        history.forEach((sample) => {
          series.forEach((key) => {
            const value = sample.counters[key];
            if (Number.isFinite(value)) {
              if (value < minValue) minValue = value;
              if (value > maxValue) maxValue = value;
            }
          });
        });

        if (!Number.isFinite(minValue) || !Number.isFinite(maxValue)) {
          renderMetricLegend([]);
          setChartEmpty(true, 'Keine Telemetriedaten gemeldet.');
          return;
        }
        if (minValue === maxValue) {
          if (minValue === 0) {
            maxValue = 1;
          } else {
            minValue = 0;
          }
        }

        const toX = (index) => {
          const ratio = index / Math.max(history.length - 1, 1);
          return padding + ratio * plotWidth;
        };
        const toY = (value) => {
          const ratio = (value - minValue) / (maxValue - minValue);
          return height - padding - ratio * plotHeight;
        };

        const firstTimestamp = history[0]?.timestamp;
        const lastTimestamp = history[history.length - 1]?.timestamp;

        const legendEntries = series.map((key, index) => ({
          key,
          value: latest[key] ?? 0,
          color: METRIC_COLORS[index % METRIC_COLORS.length],
        }));
        renderMetricLegend(legendEntries);

        ctx.save();
        const background = ctx.createLinearGradient(0, padding, 0, height - padding);
        background.addColorStop(0, 'rgba(62, 124, 214, 0.20)');
        background.addColorStop(1, 'rgba(8, 16, 30, 0.82)');
        ctx.fillStyle = background;
        ctx.fillRect(padding, padding, plotWidth, plotHeight);
        ctx.restore();

        ctx.save();
        ctx.lineWidth = 1;
        ctx.strokeStyle = 'rgba(120, 160, 220, 0.18)';
        ctx.setLineDash([4, 8]);
        const gridY = 4;
        for (let i = 1; i < gridY; i += 1) {
          const y = padding + (plotHeight / gridY) * i;
          ctx.beginPath();
          ctx.moveTo(padding, y);
          ctx.lineTo(padding + plotWidth, y);
          ctx.stroke();
        }
        const gridX = Math.min(Math.max(history.length - 1, 1), 6);
        for (let i = 1; i < gridX; i += 1) {
          const x = padding + (plotWidth / gridX) * i;
          ctx.beginPath();
          ctx.moveTo(x, padding);
          ctx.lineTo(x, padding + plotHeight);
          ctx.stroke();
        }
        ctx.restore();

        ctx.save();
        ctx.strokeStyle = 'rgba(140, 180, 240, 0.45)';
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.moveTo(padding, padding);
        ctx.lineTo(padding, height - padding);
        ctx.lineTo(width - padding, height - padding);
        ctx.stroke();
        ctx.restore();

        const hoverSeries = [];
        const timestamps = history.map((sample) => sample.timestamp ?? NaN);
        const stepX = plotWidth / Math.max(history.length - 1, 1);
        series.forEach((key, index) => {
          const color = METRIC_COLORS[index % METRIC_COLORS.length];
          const values = new Array(history.length).fill(null);
          const points = new Array(history.length).fill(null);
          const pathPoints = [];

          history.forEach((sample, idx) => {
            const value = sample.counters[key];
            if (!Number.isFinite(value)) {
              return;
            }
            const x = padding + stepX * idx;
            const y = toY(value);
            const point = { x, y };
            points[idx] = point;
            pathPoints.push(point);
            values[idx] = value;
          });

          if (pathPoints.length === 0) {
            return;
          }

          if (pathPoints.length > 1) {
            ctx.save();
            ctx.fillStyle = withAlpha(color, 0.18);
            ctx.beginPath();
            ctx.moveTo(pathPoints[0].x, height - padding);
            pathPoints.forEach((point) => ctx.lineTo(point.x, point.y));
            ctx.lineTo(pathPoints[pathPoints.length - 1].x, height - padding);
            ctx.closePath();
            ctx.fill();
            ctx.restore();
          }

          ctx.save();
          ctx.strokeStyle = color;
          ctx.lineWidth = 2.2;
          ctx.lineJoin = 'round';
          ctx.lineCap = 'round';
          ctx.beginPath();
          let started = false;
          points.forEach((point) => {
            if (!point) {
              return;
            }
            if (!started) {
              ctx.moveTo(point.x, point.y);
              started = true;
            } else {
              ctx.lineTo(point.x, point.y);
            }
          });
          ctx.stroke();
          ctx.restore();

          const lastPoint = pathPoints[pathPoints.length - 1];
          if (lastPoint) {
            ctx.save();
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.arc(lastPoint.x, lastPoint.y, 4, 0, Math.PI * 2);
            ctx.fill();
            ctx.restore();
          }

          hoverSeries.push({
            key,
            label: formatMetricLabel(key),
            color,
            values,
            points,
          });
        });

        ctx.save();
        ctx.fillStyle = 'rgba(156, 190, 255, 0.7)';
        ctx.font = '12px "Inter", system-ui, sans-serif';
        ctx.textBaseline = 'alphabetic';
        ctx.fillText(formatNumber(Math.round(maxValue)), padding + 4, padding + 12);
        ctx.fillText(formatNumber(Math.round(minValue)), padding + 4, height - padding - 4);
        ctx.textBaseline = 'top';
        ctx.textAlign = 'left';
        if (Number.isFinite(firstTimestamp)) {
          ctx.fillText(formatChartTime(firstTimestamp), padding, height - padding + 8);
        }
        if (Number.isFinite(lastTimestamp)) {
          ctx.textAlign = 'right';
          ctx.fillText(formatChartTime(lastTimestamp), width - padding, height - padding + 8);
        }
        ctx.restore();

        if (hoverSeries.length > 0) {
          chartHoverState = {
            padding,
            plotWidth,
            width,
            height,
            series: hoverSeries,
            timestamps,
          };
        } else {
          chartHoverState = null;
        }
      };

      const recordMetricSample = (counters) => {
        const numericEntries = Object.entries(counters || {}).filter(([, value]) => {
          return typeof value === 'number' && Number.isFinite(value);
        });
        if (numericEntries.length === 0) {
          metricHistory.length = 0;
          renderMetricList({});
          renderMetricChart();
          if (telemetryNote) {
            telemetryNote.textContent = 'Keine Telemetriedaten verfügbar.';
          }
          resetTelemetrySummary();
          setChartEmpty(true, 'Keine Telemetriedaten gemeldet.');
          return;
        }

        const snapshot = Object.fromEntries(numericEntries);
        const chartSnapshot = filterChartCounters(snapshot);
        if (Object.keys(chartSnapshot).length > 0) {
          const now = Date.now();
          const last = metricHistory[metricHistory.length - 1];
          if (last && Math.abs(now - last.timestamp) < 500) {
            last.timestamp = now;
            last.counters = chartSnapshot;
          } else {
            metricHistory.push({ timestamp: now, counters: chartSnapshot });
          }
          pruneMetricHistory();
        } else {
          hideChartTooltip();
        }
        renderMetricList(snapshot);
        updateTelemetrySummary(snapshot);
        renderMetricChart();
        if (telemetryNote) {
          telemetryNote.textContent = `Stand: ${new Date().toLocaleTimeString('de-DE')} · Range ${currentHistoryRange}`;
        }
      };

      const markTelemetryUnavailable = (message) => {
        metricHistory.length = 0;
        if (metricCtx && metricChartCanvas) {
          metricCtx.clearRect(0, 0, metricChartCanvas.width, metricChartCanvas.height);
        }
        renderMetricList({});
        renderMetricLegend([]);
        resetTelemetrySummary();
        hideChartTooltip();
        setChartEmpty(true, message || 'Telemetrie nicht verfügbar.');
        if (telemetryNote) {
          telemetryNote.textContent = message || 'Telemetrie nicht verfügbar.';
        }
        historyLoaded = false;
      };

      const handleChartHover = (event) => {
        if (!metricChartCanvas || !chartHoverState || !chartHoverState.timestamps || chartHoverState.timestamps.length === 0) {
          hideChartTooltip();
          return;
        }
        const rect = metricChartCanvas.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) {
          hideChartTooltip();
          return;
        }
        const scaleX = metricChartCanvas.width / rect.width;
        let canvasX = (event.clientX - rect.left) * scaleX;
        const { padding, plotWidth } = chartHoverState;
        const maxX = padding + plotWidth;
        canvasX = Math.min(Math.max(canvasX, padding), maxX);

        const total = chartHoverState.timestamps.length;
        if (total === 0) {
          hideChartTooltip();
          return;
        }
        const ratio = total === 1 ? 0 : (canvasX - padding) / plotWidth;
        let index = Math.round(ratio * (total - 1));
        index = Math.min(Math.max(index, 0), total - 1);

        renderMetricChart();
        const state = chartHoverState;
        if (!state || state.series.length === 0) {
          hideChartTooltip();
          return;
        }

        const hoverX = total === 1 ? padding : padding + (index / (total - 1)) * state.plotWidth;
        const ctx = metricCtx;
        ctx.save();
        ctx.strokeStyle = 'rgba(236, 246, 255, 0.34)';
        ctx.lineWidth = 1.5;
        ctx.setLineDash([3, 4]);
        ctx.beginPath();
        ctx.moveTo(hoverX, state.padding);
        ctx.lineTo(hoverX, state.height - state.padding);
        ctx.stroke();
        ctx.restore();

        const rows = [];
        state.series.forEach((series) => {
          const value = series.values[index];
          if (!Number.isFinite(value)) {
            return;
          }
          const point = series.points[index];
          if (point) {
            ctx.save();
            ctx.fillStyle = series.color;
            ctx.strokeStyle = 'rgba(4, 16, 32, 0.82)';
            ctx.lineWidth = 1.2;
            ctx.beginPath();
            ctx.arc(point.x, point.y, 4.5, 0, Math.PI * 2);
            ctx.fill();
            ctx.stroke();
            ctx.restore();
          }
          rows.push({
            key: series.key,
            label: series.label,
            value,
            color: series.color,
          });
        });

        if (!chartTooltip || !chartTooltipBody || !chartTooltipTime || rows.length === 0) {
          hideChartTooltip();
          return;
        }

        rows.sort((a, b) => b.value - a.value);
        chartTooltipBody.innerHTML = rows
          .map((row) => {
            const formattedValue = formatTooltipValue(row.key, row.value);
            return `
              <div class="tooltip-row">
                <span class="tooltip-swatch" style="background:${row.color}"></span>
                <span class="tooltip-label">${escapeHtml(row.label)}</span>
                <span class="tooltip-value">${formattedValue}</span>
              </div>
            `;
          })
          .join('');

        const timestamp = state.timestamps[index];
        chartTooltipTime.textContent = Number.isFinite(timestamp)
          ? formatChartTime(timestamp)
          : '–';

        chartTooltip.dataset.visible = 'true';
        const container = metricChartCanvas.parentElement;
        if (!container) {
          return;
        }
        const containerRect = container.getBoundingClientRect();
        const tooltipRect = chartTooltip.getBoundingClientRect();
        const offsetX = event.clientX - containerRect.left + 12;
        const offsetY = event.clientY - containerRect.top + 12;
        const maxLeft = containerRect.width - tooltipRect.width - 12;
        const maxTop = containerRect.height - tooltipRect.height - 12;
        const finalX = Math.max(12, Math.min(offsetX, maxLeft));
        const finalY = Math.max(12, Math.min(offsetY, maxTop));
        chartTooltip.style.transform = `translate(${finalX}px, ${finalY}px)`;
      };

      const collectServiceMetrics = () => {
        const counters = {
          'services.total': servicesCache.size,
          'services.critical': 0,
        };
        SERVICE_STATUS_KEYS.forEach((key) => {
          counters[`services.status.${key}`] = 0;
        });
        counters[SERVICE_STATUS_FALLBACK] = 0;
        SERVICE_TAG_KEYS.forEach((key) => {
          counters[`services.tag.${key}`] = 0;
        });
        servicesCache.forEach((svc) => {
          const statusKey = `services.status.${svc.status ?? 'other'}`;
          if (statusKey in counters) {
            counters[statusKey] += 1;
          } else {
            counters[SERVICE_STATUS_FALLBACK] += 1;
          }
          if (Array.isArray(svc.tags)) {
            svc.tags.forEach((tag) => {
              const tagKey = `services.tag.${tag}`;
              if (tagKey in counters) {
                counters[tagKey] += 1;
              }
            });
          }
          if (svc.critical) {
            counters['services.critical'] += 1;
          }
        });
        return counters;
      };

      const pushServiceMetricsSample = () => {
        if (!servicesCache || servicesCache.size === 0) {
          return;
        }
        renderMetricList(collectServiceMetrics());
      };

      const getStoredPage = () => {
        try {
          const value = localStorage.getItem(PAGE_STORAGE_KEY);
          if (value && pageContainers.has(value)) {
            return value;
          }
        } catch (_) {}
        return 'overview';
      };

      const setActivePage = (name, { persist = true } = {}) => {
        const target = pageContainers.has(name) ? name : 'overview';
        pageContainers.forEach((element, key) => {
          element.dataset.visible = key === target ? 'true' : 'false';
        });
        pageButtons.forEach((button) => {
          const active = button.dataset.pageTrigger === target;
          button.dataset.active = active ? 'true' : 'false';
          button.setAttribute('aria-selected', active ? 'true' : 'false');
        });
        if (persist) {
          try {
            localStorage.setItem(PAGE_STORAGE_KEY, target);
          } catch (_) {}
        }
        if (target === 'telemetry') {
          window.requestAnimationFrame(() => renderMetricChart());
          if (!historyLoaded && !historyLoading) {
            fetchTelemetryHistory(currentHistoryRange, { background: true }).catch(() => {});
          }
        } else {
          hideChartTooltip();
        }
      };

      resetTelemetrySummary();
      setChartEmpty(true, 'Noch keine Telemetriedaten verfügbar.');

      const initialPage = getStoredPage();
      setActivePage(initialPage, { persist: false });

      pageButtons.forEach((button) => {
        button.addEventListener('click', () => {
          const target = button.dataset.pageTrigger || 'overview';
          setActivePage(target);
        });
      });

      setHistoryButtonsActive(currentHistoryRange);
      historyRangeButtons.forEach((button) => {
        button.addEventListener('click', () => {
          const targetRange = button.dataset.historyRange;
          if (!targetRange || targetRange === currentHistoryRange || historyLoading) {
            return;
          }
          currentHistoryRange = targetRange;
          setHistoryButtonsActive(targetRange);
          historyLoaded = false;
          fetchTelemetryHistory(targetRange).catch(() => {});
        });
      });

      setAuditButtonsActive(currentAuditRange);
      auditRangeButtons.forEach((button) => {
        button.addEventListener('click', () => {
          const targetRange = button.dataset.auditRange;
          if (!targetRange || targetRange === currentAuditRange || auditHistoryLoading) {
            return;
          }
          currentAuditRange = targetRange;
          setAuditButtonsActive(targetRange);
          auditHistoryLoaded = false;
          fetchAuditHistory(targetRange).catch(() => {});
        });
      });

      if (metricChartCanvas) {
        metricChartCanvas.addEventListener('mousemove', handleChartHover);
        metricChartCanvas.addEventListener('mouseleave', () => {
          hideChartTooltip();
          if (chartHoverState) {
            window.requestAnimationFrame(() => renderMetricChart());
          }
        });
      }

      const renderActor = (actor) => {
        if (!actor || actor.kind === 'system') {
          return '<span class="metadata-chip">system</span>';
        }
        const chips = [];
        if (actor.role) {
          chips.push(`<span class="metadata-chip">role=${actor.role}</span>`);
        }
        if (actor.user_id) {
          chips.push(`<span class="metadata-chip">user=${actor.user_id}</span>`);
        } else if (actor.user_id_redacted) {
          chips.push('<span class="metadata-chip" data-redacted="true">user=&lt;redacted&gt;</span>');
        }
        return chips.join('');
      };

      const renderMetadata = (metadata) => {
        if (!metadata || metadata.length === 0) {
          return '<span class="metadata-chip" data-redacted="false">none</span>';
        }
        return metadata
          .map((entry) => {
            const safeValue = entry.value || '–';
            return `<span class="metadata-chip" data-redacted="${entry.redacted}">${entry.key}=${safeValue}</span>`;
          })
          .join('');
      };

      const renderAuditCache = () => {
        if (!auditCache || auditCache.length === 0) {
          auditBody.innerHTML = '<tr><td colspan="6">Keine Audit-Ereignisse vorhanden.</td></tr>';
          auditMeta.textContent = `0 Einträge · Range ${currentAuditRange}`;
          return;
        }
        auditMeta.textContent = `${auditCache.length} Einträge · Range ${currentAuditRange}`;
        auditBody.innerHTML = auditCache
          .map((event) => {
            const outcomeClass = event.outcome === 'success'
              ? 'pill success'
              : event.outcome === 'denied'
              ? 'pill denied'
              : 'pill failure';
            return `
              <tr>
                <td>${formatTimestamp(event.timestamp)}</td>
                <td>${event.action}</td>
                <td>${event.target}</td>
                <td><span class="${outcomeClass}">${event.outcome}</span></td>
                <td><div class="metadata-chips">${renderActor(event.actor)}</div></td>
                <td><div class="metadata-chips">${renderMetadata(event.metadata)}</div></td>
              </tr>
            `;
          })
          .join('');
      };

      const renderAuditEvents = (payload) => {
        auditCache = payload?.events ?? [];
        renderAuditCache();
      };

      const handleAuditPush = (event) => {
        if (!event) {
          return;
        }
        const eventTime = Date.parse(event.timestamp ?? '');
        const rangeMs = rangeToMillis(currentAuditRange);
        const cutoff = Date.now() - rangeMs;
        if (Number.isFinite(eventTime) && eventTime < cutoff) {
          return;
        }

        const key = `${event.timestamp}|${event.action}|${event.target}`;
        const existingIndex = auditCache.findIndex((entry) => {
          return `${entry.timestamp}|${entry.action}|${entry.target}` === key;
        });
        if (existingIndex !== -1) {
          auditCache.splice(existingIndex, 1);
        }
        auditCache.unshift(event);
        trimAuditCache();
        renderAuditCache();
      };

      const connectEventStream = () => {
        if (!window.EventSource) {
          return;
        }
        if (eventSource) {
          eventSource.close();
          eventSource = null;
        }
        const token = currentToken();
        const url = token
          ? `/events/stream?token=${encodeURIComponent(token)}`
          : '/events/stream';
        try {
          eventSource = new EventSource(url);
        } catch (error) {
          if (!sseWarned) {
            showAlert('Event-Stream konnte nicht aufgebaut werden.');
            sseWarned = true;
          }
          return;
        }
        sseWarned = false;
        eventSource.addEventListener('audit', (event) => {
          try {
            const payload = JSON.parse(event.data);
            handleAuditPush(payload);
          } catch (error) {
            console.warn('Fehler beim Verarbeiten von Audit-SSE', error);
          }
        });
        eventSource.addEventListener('service-state', (event) => {
          try {
            const payload = JSON.parse(event.data);
            applyServiceState(payload);
          } catch (error) {
            console.warn('Fehler beim Verarbeiten von Service-SSE', error);
          }
        });
        eventSource.addEventListener('audit-error', (event) => {
          if (!sseWarned) {
            showAlert(event.data || 'Event-Stream verweigert. Bitte Token prüfen.');
            sseWarned = true;
          }
        });
        eventSource.addEventListener('error', () => {
          if (!sseWarned) {
            showAlert('Event-Stream unterbrochen – Fallback auf Polling.');
            sseWarned = true;
          }
        });
      };

      const updateUptimeTicker = () => {
        if (uptimeBaseSeconds == null || uptimeAnchor == null) {
          uptimeEl.textContent = '–';
          return;
        }
        const elapsedSeconds = Math.max(0, Math.floor((Date.now() - uptimeAnchor) / 1000));
        uptimeEl.textContent = formatDuration(uptimeBaseSeconds + elapsedSeconds);
      };

      const setUptimeBase = (seconds) => {
        if (seconds == null) {
          uptimeBaseSeconds = null;
          uptimeAnchor = null;
          uptimeEl.textContent = '–';
          if (uptimeHandle !== null) {
            window.clearInterval(uptimeHandle);
            uptimeHandle = null;
          }
          return;
        }
        uptimeBaseSeconds = seconds;
        uptimeAnchor = Date.now();
        updateUptimeTicker();
        if (uptimeHandle === null) {
          uptimeHandle = window.setInterval(updateUptimeTicker, UPTIME_TICK_MS);
        }
      };

      const statusClass = (status) => {
        switch (status) {
          case 'active':
            return 'status-pill status-active';
          case 'starting':
            return 'status-pill status-starting';
          case 'degraded':
            return 'status-pill status-degraded';
          case 'failed':
            return 'status-pill status-failed';
          case 'stopped':
            return 'status-pill status-stopped';
          default:
            return 'status-pill status-starting';
        }
      };

      const renderTags = (tags) => {
        if (!tags || tags.length === 0) {
          return '<span class="tag">none</span>';
        }
        return tags
          .map((tag) => {
            let cls = 'tag';
            if (tag === 'core') cls += ' tag-core';
            if (tag === 'platform') cls += ' tag-platform';
            return `<span class="${cls}">${tag}</span>`;
          })
          .join('');
      };

      const renderRowActions = (svc) => {
        const disabled = svc.tags.includes('core');
        const disableAttr = disabled ? 'disabled' : '';
        return `
          <div class="row-actions">
            <button type="button" data-service-action="start" data-service-id="${svc.id}">Start</button>
            <button type="button" data-service-action="stop" data-service-id="${svc.id}" ${disableAttr}>Stop</button>
            <button type="button" data-service-action="restart" data-service-id="${svc.id}" ${disableAttr}>Restart</button>
          </div>
        `;
      };

      const updateMeta = (info) => {
        appName.textContent = info.app?.name ?? 'Fenrir';
        appVersion.textContent = `Version ${info.app?.version ?? 'unbekannt'}`;
        const host = info.http?.host ?? 'localhost';
        const port = info.http?.port ?? 'n/a';
        metaEl.innerHTML = `
          <span>${host}:${port}</span>
          <span>Control Plane</span>
        `;
      };

      const renderServicesTable = () => {
        if (servicesCache.size === 0) {
          svcBody.innerHTML = '<tr><td colspan="6">Keine Services registriert.</td></tr>';
          metaServices.textContent = '0 Services';
          return;
        }
        const items = Array.from(servicesCache.values()).sort((a, b) =>
          (a.id || '').localeCompare(b.id || ''),
        );
        metaServices.textContent = `${items.length} Services`;
        svcBody.innerHTML = items
          .map((svc) => {
            const status = svc.status ?? 'unknown';
            const note = svc.note ?? '–';
            const tags = renderTags(svc.tags ?? []);
            return `
              <tr>
                <td class="service-id">${svc.id}</td>
                <td class="service-name">${svc.name ?? svc.id}</td>
                <td class="status-cell"><span class="${statusClass(status)}" title="${status}">${status}</span></td>
                <td class="tags-cell"><div class="tag-list">${tags}</div></td>
                <td class="note-cell">${note}</td>
                <td>${renderRowActions(svc)}</td>
              </tr>
            `;
          })
          .join('');
      };

      const updateServices = (payload) => {
        servicesCache = new Map();
        if (payload?.services && Array.isArray(payload.services)) {
          payload.services.forEach((svc) => {
            servicesCache.set(svc.id, {
              ...svc,
              note: svc.note ?? '–',
              tags: Array.isArray(svc.tags) ? svc.tags : [],
            });
          });
        }
        renderServicesTable();
        pushServiceMetricsSample();
      };

      const applyServiceState = (event) => {
        if (!event || !event.id) {
          return;
        }
        const existing = servicesCache.get(event.id) || {};
        const tags = Array.isArray(event.tags) ? event.tags : existing.tags ?? [];
        const updated = {
          ...existing,
          id: event.id,
          name: event.name ?? existing.name ?? event.id,
          kind: event.kind ?? existing.kind ?? 'other',
          status: event.status ?? existing.status ?? 'unknown',
          note: event.note ?? existing.note ?? '–',
          tags,
          critical: event.critical ?? existing.critical ?? false,
        };
        servicesCache.set(event.id, updated);
        renderServicesTable();
        pushServiceMetricsSample();
      };

      const updateHealth = (live, ready) => {
        if (live && ready) {
          healthValue.textContent = 'bereit';
          healthNote.textContent = 'System lebt & ist einsatzbereit.';
        } else if (live && !ready) {
          healthValue.textContent = 'initialisiert';
          healthNote.textContent = 'Liveness ok, Ready-Checks ausstehend.';
        } else {
          healthValue.textContent = 'nicht erreichbar';
          healthNote.textContent = 'Bitte Logs prüfen oder Admin informieren.';
        }
      };

      const fetchWithToken = (url, options = {}) => {
        const headers = { ...options.headers, ...authHeaders() };
        return fetch(url, { ...options, headers });
      };

      const performServiceAction = async (id, action, force = false) => {
        const url = `/services/${id}/${action}`;
        const options = { method: 'POST' };
        if (action !== 'start') {
          options.headers = { 'Content-Type': 'application/json' };
          options.body = JSON.stringify({ force });
        }
        const response = await fetchWithToken(url, options);
        if (!response.ok) {
          const body = await response.json().catch(() => ({}));
          throw new Error(body?.message ?? `Aktion fehlgeschlagen (${response.status})`);
        }
        return response.json();
      };

      const performBulkAction = async (kind, force = false) => {
        const options = {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ force }),
        };
        if (kind === 'start-all') {
          delete options.body;
          delete options.headers['Content-Type'];
        }
        const response = await fetchWithToken(`/services/actions/${kind}`, options);
        if (!response.ok) {
          const body = await response.json().catch(() => ({}));
          throw new Error(body?.message ?? `Aktion fehlgeschlagen (${response.status})`);
        }
        return response.json();
      };

      const loadAll = async ({ background = false } = {}) => {
        if (isLoading) {
          return;
        }
        isLoading = true;
        if (!background) {
          showAlert('');
        }
        try {
          const [infoRes, servicesRes, metricsRes] = await Promise.all([
            fetchWithToken('/info'),
            fetchWithToken('/services'),
            fetchWithToken('/metrics'),
          ]);

          if (infoRes.ok) {
            updateMeta(await infoRes.json());
          }

          if (servicesRes.ok) {
            updateServices(await servicesRes.json());
          } else {
            servicesCache = new Map();
            svcBody.innerHTML = '<tr><td colspan="6">Fehler beim Laden der Services.</td></tr>';
            metaServices.textContent = `Fehler (${servicesRes.status})`;
          }

          if (metricsRes.ok) {
            const metrics = await metricsRes.json();
            if (!historyLoaded) {
              await fetchTelemetryHistory(currentHistoryRange, { background });
            }
            setUptimeBase(metrics.uptime_seconds ?? null);
            updateHealth(metrics.live, metrics.ready);
            recordMetricSample(metrics.counters ?? {});
          } else {
            setUptimeBase(null);
            healthValue.textContent = 'unbekannt';
            healthNote.textContent = 'Telemetrie nicht verfügbar.';
            markTelemetryUnavailable(`Telemetrie nicht verfügbar (${metricsRes.status}).`);
          }

          if (!auditHistoryLoaded && !auditHistoryLoading) {
            fetchAuditHistory(currentAuditRange, { background: background || !document.hasFocus() }).catch(() => {});
          }
        } catch (error) {
          servicesCache = new Map();
          svcBody.innerHTML = '<tr><td colspan="6">Netzwerkfehler: Daten konnten nicht geladen werden.</td></tr>';
          metaServices.textContent = 'Fehler';
          showAlert(background ? 'Auto-Refresh fehlgeschlagen – Verbindung prüfen.' : 'Netzwerkfehler: Bitte Verbindung prüfen.');
          auditBody.innerHTML = '<tr><td colspan="6">Netzwerkfehler – keine Audit-Daten.</td></tr>';
          auditMeta.textContent = 'Fehler';
          auditHistoryLoaded = false;
          setUptimeBase(null);
          healthValue.textContent = 'unbekannt';
          healthNote.textContent = 'Telemetrie nicht verfügbar.';
          markTelemetryUnavailable('Telemetrie nicht verfügbar.');
        } finally {
          isLoading = false;
        }
      };

      const scheduleRefresh = () => {
        if (refreshHandle !== null) {
          window.clearInterval(refreshHandle);
        }
        refreshHandle = window.setInterval(() => {
          if (document.hidden) {
            return;
          }
          loadAll({ background: true });
        }, REFRESH_INTERVAL_MS);
      };

      tokenInput.addEventListener('change', () => {
        const token = currentToken();
        saveToken(token);
        setTokenUi(token);
        connectEventStream();
      });

      if (auditRefreshBtn) {
        auditRefreshBtn.addEventListener('click', () => {
          auditHistoryLoaded = false;
          historyLoaded = false;
          loadAll({ background: true });
        });
      }

      testButton.addEventListener('click', async () => {
        testButton.disabled = true;
        testButton.textContent = 'prüfe …';
        try {
          const response = await fetchWithToken('/services');
          if (response.ok) {
            tokenStatus.textContent = 'Token gültig – Zugriff erlaubt.';
          } else if (response.status === 401) {
            tokenStatus.textContent = 'Token ungültig – Autorisierung fehlgeschlagen (401).';
          } else if (response.status === 403) {
            tokenStatus.textContent = 'Token besitzt nicht ausreichende Rolle (403).';
          } else {
            tokenStatus.textContent = `Anfrage fehlgeschlagen (Status ${response.status}).`;
          }
        } catch (_) {
          tokenStatus.textContent = 'Netzwerkfehler – Anfrage konnte nicht gesendet werden.';
        } finally {
          setTimeout(() => {
            testButton.textContent = 'Token Test';
            testButton.disabled = currentToken() === '';
          }, 650);
        }
      });

      const handleBulk = async (kind) => {
        try {
          showAlert('');
          const destructive = kind !== 'start-all';
          if (destructive && !window.confirm('Aktion wirklich ausführen?')) {
            return;
          }
          const result = await performBulkAction(kind, destructive);
          const successes = result.results.filter((r) => r.status === 'success').length;
          const failures = result.results.length - successes;
          showAlert(
            `Aktion ${result.action} abgeschlossen: ${successes} erfolgreich, ${failures} fehlgeschlagen.`,
          );
          await loadAll();
        } catch (error) {
          showAlert(error.message || 'Bulk-Aktion fehlgeschlagen.');
        }
      };

      bulkStartBtn.addEventListener('click', () => handleBulk('start-all'));
      bulkStopBtn.addEventListener('click', () => handleBulk('stop-all'));
      bulkRestartBtn.addEventListener('click', () => handleBulk('restart-all'));

      svcBody.addEventListener('click', async (event) => {
        const target = event.target;
        if (!(target instanceof HTMLElement)) {
          return;
        }
        const action = target.dataset.serviceAction;
        const id = target.dataset.serviceId;
        if (!action || !id) {
          return;
        }
        if (target.disabled) {
          return;
        }
        try {
          const destructive = action !== 'start';
          if (destructive && !window.confirm(`Aktion ${action} für ${id} wirklich ausführen?`)) {
            return;
          }
          target.disabled = true;
          await performServiceAction(id, action, destructive);
          showAlert(`Service ${id}: Aktion ${action} erfolgreich.`);
          await loadAll();
        } catch (error) {
          showAlert(error.message || `Aktion ${action} fehlgeschlagen.`);
        } finally {
          target.disabled = false;
        }
      });

      const init = async () => {
        const token = loadToken();
        tokenInput.value = token;
        setTokenUi(token);
        connectEventStream();
        await loadAll();
        scheduleRefresh();
        document.addEventListener('visibilitychange', () => {
          if (!document.hidden) {
            loadAll({ background: true });
          }
        });
        window.addEventListener('focus', () => loadAll({ background: true }));
      };

      init();

    </script>
  </body>
</html>
"#;
