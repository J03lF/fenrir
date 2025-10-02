use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
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
use std::convert::Infallible;
use tokio::net::{lookup_host, TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tower::service_fn;
use tower::util::ServiceExt;
use tracing::{debug, info, warn};

use crate::config::AppConfig;
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
struct ServiceActionErrorBody {
    error: &'static str,
    message: String,
}

struct ServiceActionProblem {
    status: StatusCode,
    body: ServiceActionErrorBody,
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
        .route("/scheduler/jobs", get(list_scheduler_jobs))
        .route("/logging/level", post(update_logging_level))
        .route("/metrics", get(metrics_snapshot))
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

    let token = extract_bearer_token(&headers);
    let role = auth.authorize_token(token).map_err(map_auth_error)?;
    let required = kind.required_role();
    if !role.satisfies(required.clone()) {
        return Err(ServiceActionProblem::new(
            StatusCode::FORBIDDEN,
            "role_insufficient",
            format!(
                "Aktion erfordert Rolle {required:?}, aktuelle Rolle {role:?} reicht nicht aus"
            ),
        ));
    }

    let outcome = match kind {
        ServiceActionKind::Start => services
            .start_service(&id)
            .map_err(|err| map_service_control_error(err, &id))?,
        ServiceActionKind::Stop => services
            .stop_service(&id, force)
            .map_err(|err| map_service_control_error(err, &id))?,
        ServiceActionKind::Restart => services
            .restart_service(&id, force)
            .map_err(|err| map_service_control_error(err, &id))?,
    };

    let snapshot = registry.get(&id).map(snapshot_to_summary);

    Ok(ServiceActionResponse {
        id,
        action: kind.as_str(),
        outcome: outcome.as_str(),
        force,
        service: snapshot,
        actor_role: role.as_str(),
    })
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
        background: radial-gradient(circle at top, #132035 0%, #09121f 52%, #050a13 100%);
        color: #e7eefb;
      }
      body {
        margin: 0;
        min-height: 100vh;
        display: flex;
        align-items: stretch;
        justify-content: center;
        padding: 3.5rem 1.5rem 4.5rem;
      }
      main.shell {
        width: min(1080px, 100%);
        display: grid;
        gap: 2.2rem;
      }
      header.navbar {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 0.9rem 1.2rem;
        border-radius: 1rem;
        background: rgba(10, 20, 33, 0.72);
        border: 1px solid rgba(120, 160, 220, 0.18);
        box-shadow: 0 18px 34px rgba(4, 10, 18, 0.32);
      }
      header.navbar .brand {
        display: flex;
        align-items: center;
        gap: 0.75rem;
      }
      header.navbar .brand span.logo {
        width: 42px;
        height: 42px;
        border-radius: 12px;
        background: linear-gradient(135deg, #3b81f6 0%, #34d3c7 100%);
        display: grid;
        place-items: center;
        font-weight: 700;
        letter-spacing: 0.08em;
        color: #08101c;
      }
      header.navbar .brand h1 {
        margin: 0;
        font-size: 1.2rem;
        letter-spacing: 0.08em;
        text-transform: uppercase;
      }
      header.navbar .actions {
        display: flex;
        align-items: center;
        gap: 0.75rem;
      }
      header.navbar .actions button {
        appearance: none;
        border: 0;
        border-radius: 999px;
        padding: 0.48rem 1.1rem;
        font-size: 0.85rem;
        letter-spacing: 0.08em;
        text-transform: uppercase;
        background: linear-gradient(135deg, rgba(59, 129, 246, 0.95), rgba(52, 211, 199, 0.95));
        color: #07101b;
        cursor: pointer;
        font-weight: 600;
        transition: filter 0.2s ease;
      }
      header.navbar .actions button:disabled {
        filter: grayscale(1) opacity(0.6);
        cursor: not-allowed;
      }
      section.hero {
        display: grid;
        gap: 1.3rem;
      }
      h2.title {
        margin: 0;
        font-size: clamp(2.3rem, 5vw, 3rem);
        letter-spacing: -0.02em;
      }
      p.subtitle {
        margin: 0;
        font-size: 1.1rem;
        color: #9ab1d0;
        max-width: 60ch;
      }
      .meta {
        display: flex;
        flex-wrap: wrap;
        gap: 0.7rem;
        font-size: 0.92rem;
        color: #84a3c9;
      }
      .meta span {
        padding: 0.45rem 0.85rem;
        border-radius: 999px;
        background: rgba(106, 149, 203, 0.18);
        border: 1px solid rgba(126, 172, 228, 0.24);
      }
      section.cards {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
        gap: 1.35rem;
      }
      article.card {
        padding: 1.55rem;
        border-radius: 1.2rem;
        background: rgba(15, 24, 39, 0.78);
        backdrop-filter: blur(12px);
        border: 1px solid rgba(104, 150, 211, 0.24);
        box-shadow: 0 24px 48px rgba(3, 8, 16, 0.35);
        display: grid;
        gap: 0.75rem;
        min-height: 150px;
      }
      .card h3 {
        margin: 0;
        font-size: 1rem;
        text-transform: uppercase;
        letter-spacing: 0.12em;
        color: #7ea6db;
      }
      .card .value {
        font-size: 1.9rem;
        font-weight: 600;
      }
      .card .hint {
        font-size: 0.9rem;
        color: #9db6d6;
      }
      .token-card {
        display: grid;
        gap: 0.75rem;
      }
      .token-input {
        display: flex;
        gap: 0.6rem;
        flex-wrap: wrap;
      }
      .token-input input {
        flex: 1 1 260px;
        padding: 0.6rem 0.85rem;
        border-radius: 0.9rem;
        border: 1px solid rgba(134, 170, 225, 0.34);
        background: rgba(12, 20, 31, 0.92);
        color: #e6f0ff;
        font-size: 0.95rem;
        letter-spacing: 0.03em;
      }
      .token-input input:focus {
        outline: none;
        border-color: rgba(82, 150, 255, 0.55);
        box-shadow: 0 0 0 3px rgba(82, 150, 255, 0.18);
      }
      .token-status {
        font-size: 0.85rem;
        color: #8fbce0;
      }
      section.services {
        display: grid;
        gap: 1.25rem;
      }
      .services header {
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        gap: 1rem;
      }
      .services h2 {
        margin: 0;
        font-size: 1.35rem;
      }
      .services small {
        color: #7e9dc4;
      }
      table.service-table {
        width: 100%;
        border-collapse: collapse;
        border-radius: 1rem;
        overflow: hidden;
        background: rgba(11, 19, 30, 0.82);
        border: 1px solid rgba(106, 154, 213, 0.18);
        box-shadow: 0 18px 34px rgba(5, 10, 18, 0.38);
      }
      table.service-table thead {
        background: rgba(18, 42, 73, 0.55);
      }
      table.service-table th,
      table.service-table td {
        padding: 0.95rem 1.1rem;
        text-align: left;
        font-size: 0.95rem;
      }
      table.service-table tbody tr + tr {
        border-top: 1px solid rgba(123, 166, 224, 0.12);
      }
      .status-pill {
        display: inline-flex;
        align-items: center;
        gap: 0.4rem;
        font-weight: 600;
        letter-spacing: 0.04em;
        padding: 0.3rem 0.75rem;
        border-radius: 999px;
        text-transform: uppercase;
        font-size: 0.78rem;
      }
      .status-active {
        background: rgba(71, 209, 167, 0.16);
        color: #49f1bd;
      }
      .status-starting {
        background: rgba(224, 176, 49, 0.18);
        color: #f4c76a;
      }
      .status-degraded {
        background: rgba(214, 120, 20, 0.22);
        color: #ffb46b;
      }
      .status-failed {
        background: rgba(207, 60, 82, 0.22);
        color: #ff8aa1;
      }
      .status-stopped {
        background: rgba(113, 137, 169, 0.2);
        color: #9fb4d4;
      }
      .tag-list {
        display: inline-flex;
        flex-wrap: wrap;
        gap: 0.4rem;
      }
      .tag {
        padding: 0.25rem 0.6rem;
        border-radius: 999px;
        font-size: 0.78rem;
        background: rgba(130, 167, 221, 0.18);
        border: 1px solid rgba(130, 167, 221, 0.28);
        letter-spacing: 0.04em;
      }
      .tag-core {
        color: #ffd384;
        border-color: rgba(255, 208, 122, 0.38);
        background: rgba(255, 208, 122, 0.16);
      }
      .tag-platform {
        color: #8cc6ff;
      }
      footer.note {
        text-align: center;
        font-size: 0.83rem;
        color: rgba(139, 167, 204, 0.78);
      }
      @media (max-width: 640px) {
        body {
          padding-top: 2.5rem;
        }
        header.navbar {
          flex-direction: column;
          align-items: stretch;
          gap: 0.8rem;
        }
        header.navbar .actions {
          justify-content: flex-end;
        }
        table.service-table th:nth-child(4),
        table.service-table td:nth-child(4),
        table.service-table th:nth-child(5),
        table.service-table td:nth-child(5) {
          display: none;
        }
        .token-input {
          flex-direction: column;
        }
      }
    </style>
  </head>
  <body>
    <main class="shell">
      <header class="navbar">
        <div class="brand">
          <span class="logo">F</span>
          <h1>Fenrir Control Plane</h1>
        </div>
        <div class="actions">
          <button type="button" data-test-btn disabled>API Test</button>
        </div>
      </header>

      <section class="hero">
        <h2 class="title">Fenrir Operations Portal</h2>
        <p class="subtitle">
          Transparente Übersicht über Transports, Scheduler und Services des Fenrir Ticket-Backends.
          Aktionen werden auditierbar und rollenbasiert abgesichert.
        </p>
        <div class="meta" data-meta>
          <span>lade Applikationsdaten …</span>
        </div>
      </section>

      <section class="cards">
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
          <div class="token-status" data-token-status>Token nicht gesetzt – Anfragen erfolgen ohne Auth.</div>
        </article>
        <article class="card" data-card="app">
          <h3>Applikation</h3>
          <div class="value" data-app-name>–</div>
          <div class="hint" data-app-version>Version wird geladen …</div>
        </article>
        <article class="card" data-card="uptime">
          <h3>Uptime</h3>
          <div class="value" data-uptime>–</div>
          <div class="hint">Quelle: Telemetrie-Snapshot</div>
        </article>
        <article class="card" data-card="health">
          <h3>Status</h3>
          <div class="value" data-health>—</div>
          <div class="hint" data-health-note>Prüfe Readiness-Status …</div>
        </article>
      </section>

      <section class="services">
        <header>
          <h2>Registrierte Services</h2>
          <small data-service-meta>lade Registry …</small>
        </header>
        <table class="service-table">
          <thead>
            <tr>
              <th>ID</th>
              <th>Name</th>
              <th>Status</th>
              <th>Tags</th>
              <th>Hinweis</th>
            </tr>
          </thead>
          <tbody data-services>
            <tr>
              <td colspan="5">Service-Registry wird abgefragt …</td>
            </tr>
          </tbody>
        </table>
      </section>

      <footer class="note">
        Zugriff auf erweiterte Aktionen erfolgt über die CLI oder autorisierte API-Clients.
        Audit-Logs halten administrative Eingriffe nach.
      </footer>
    </main>

    <script>
      const metaEl = document.querySelector('[data-meta]');
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

      const TOKEN_KEY = 'fenrir-control-plane-token';

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
        } catch (_) {
          // storage might be unavailable; ignore.
        }
      };

      const currentToken = () => tokenInput.value.trim();

      const authHeaders = () => {
        const token = currentToken();
        if (!token) {
          return {};
        }
        return { Authorization: token.startsWith('Bearer ') ? token : `Bearer ${token}` };
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

      const updateServices = (payload) => {
        if (!payload?.services) {
          svcBody.innerHTML = '<tr><td colspan="5">Keine Services registriert.</td></tr>';
          metaServices.textContent = '0 Services';
          return;
        }
        const items = payload.services;
        metaServices.textContent = `${items.length} Services`;
        svcBody.innerHTML = items
          .map((svc) => {
            const status = svc.status ?? 'unknown';
            const note = svc.note ?? '–';
            const tags = renderTags(svc.tags ?? []);
            return `
              <tr>
                <td>${svc.id}</td>
                <td>${svc.name}</td>
                <td><span class="${statusClass(status)}">${status}</span></td>
                <td class="tag-list">${tags}</td>
                <td>${note}</td>
              </tr>
            `;
          })
          .join('');
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

      const fetchWithToken = async (url) => {
        const headers = authHeaders();
        return fetch(url, {
          headers,
        });
      };

      const bootstrap = async () => {
        const token = loadToken();
        tokenInput.value = token;
        setTokenUi(token);

        try {
          const [infoRes, servicesRes, metricsRes] = await Promise.all([
            fetchWithToken('/info'),
            fetchWithToken('/services'),
            fetchWithToken('/metrics'),
          ]);

          if (infoRes.ok) {
            const info = await infoRes.json();
            updateMeta(info);
          }

          if (servicesRes.ok) {
            const services = await servicesRes.json();
            updateServices(services);
          } else {
            svcBody.innerHTML = '<tr><td colspan="5">Fehler beim Laden der Services.</td></tr>';
            metaServices.textContent = 'Fehler';
          }

          if (metricsRes.ok) {
            const metrics = await metricsRes.json();
            uptimeEl.textContent = formatDuration(metrics.uptime_seconds);
            updateHealth(metrics.live, metrics.ready);
          } else {
            uptimeEl.textContent = '–';
            healthValue.textContent = 'unbekannt';
            healthNote.textContent = 'Telemetrie nicht verfügbar.';
          }
        } catch (error) {
          svcBody.innerHTML = '<tr><td colspan="5">Netzwerkfehler: Daten konnten nicht geladen werden.</td></tr>';
          metaServices.textContent = 'Fehler';
          uptimeEl.textContent = '–';
          healthValue.textContent = 'unbekannt';
          healthNote.textContent = 'Netzwerkverbindung prüfen.';
          console.warn('control-plane bootstrap failed', error);
        }
      };

      tokenInput.addEventListener('change', () => {
        const token = currentToken();
        saveToken(token);
        setTokenUi(token);
      });

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
        } catch (error) {
          tokenStatus.textContent = 'Netzwerkfehler – Anfrage konnte nicht gesendet werden.';
        } finally {
          setTimeout(() => {
            testButton.textContent = 'API Test';
            testButton.disabled = currentToken() === '';
          }, 600);
        }
      });

      bootstrap();
    </script>
  </body>
</html>
"#;
