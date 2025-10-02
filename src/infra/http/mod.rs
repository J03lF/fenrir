use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock, Weak};
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
use hyper::body::Incoming;
use hyper::{Request as HyperRequest, Response as HyperResponse};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as HyperBuilder;
use hyper_util::service::TowerToHyperService;
use http_body_util::BodyExt;
use rustls::{self, ServerConfig as RustlsServerConfig};
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tokio::net::{lookup_host, TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info, warn};
use tower::service_fn;
use tower::util::ServiceExt;

use crate::config::AppConfig;
use crate::infra::telemetry;
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
        let mut maybe_update: Option<(Arc<HttpTlsProvider>, HttpTlsRuntime)> = None;
        {
            let mut guard = self
                .tls_provider
                .write()
                .map_err(|_| anyhow!("tls provider lock poisoned"))?;
            if new_runtime.enabled {
                if let Some(provider) = guard.as_ref() {
                    maybe_update = Some((Arc::clone(provider), new_runtime));
                } else {
                    let provider = Arc::new(HttpTlsProvider::new(new_runtime.clone())?);
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
}

impl HttpTlsProvider {
    fn new(runtime: HttpTlsRuntime) -> Result<Self> {
        let config = load_server_config_sync(&runtime)?;
        let provider = Self {
            next_reload: Mutex::new(
                runtime
                    .reload_interval
                    .map(|interval| Instant::now() + interval),
            ),
            runtime: RwLock::new(runtime),
            config: ArcSwap::from_pointee(config),
        };
        Ok(provider)
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
        builder.serve_connection(io, svc).await.map_err(|e| anyhow!(e))?;
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

    async fn update_runtime(&self, runtime: HttpTlsRuntime) -> Result<()> {
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
        Ok(())
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
            "TLS_CHACHA20_POLY1305_SHA256" => Ok(rustls::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256),
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
        background: radial-gradient(circle at top, #1d2a3a 0%, #0b1018 60%, #05070c 100%);
        color: #e6edf6;
      }
      body {
        margin: 0;
        padding: 3rem 1.5rem 4rem;
        display: flex;
        justify-content: center;
      }
      main.layout {
        max-width: 960px;
        width: 100%;
        display: grid;
        gap: 2.5rem;
      }
      .hero {
        display: flex;
        flex-direction: column;
        gap: 1rem;
      }
      .hero h1 {
        font-size: 2.75rem;
        letter-spacing: -0.015em;
        margin: 0;
      }
      .hero .subtitle {
        font-size: 1.1rem;
        color: #9bb3cc;
        margin: 0;
      }
      .meta {
        display: flex;
        flex-wrap: wrap;
        gap: 0.75rem;
        align-items: center;
        color: #7ea0c7;
        font-size: 0.95rem;
      }
      .meta span {
        background: rgba(111, 150, 203, 0.16);
        padding: 0.4rem 0.75rem;
        border-radius: 999px;
      }
      .cards {
        display: grid;
        gap: 1rem;
        grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
      }
      .token-panel {
        display: grid;
        gap: 0.5rem;
        padding: 1.25rem 1.5rem;
        border-radius: 1rem;
        background: rgba(17, 26, 38, 0.72);
        border: 1px solid rgba(113, 156, 201, 0.24);
        box-shadow: 0 18px 40px rgba(5, 10, 18, 0.35);
      }
      .token-panel label {
        font-size: 0.95rem;
        text-transform: uppercase;
        letter-spacing: 0.12em;
        color: #7aa6d9;
      }
      .token-input-group {
        display: flex;
        flex-wrap: wrap;
        gap: 0.6rem;
        align-items: center;
      }
      .token-input-group input {
        flex: 1 1 240px;
        background: rgba(12, 18, 28, 0.85);
        border: 1px solid rgba(120, 158, 205, 0.45);
        border-radius: 0.6rem;
        padding: 0.45rem 0.75rem;
        color: #e6edf6;
        font-size: 0.95rem;
      }
      .card {
        background: rgba(17, 26, 38, 0.72);
        border: 1px solid rgba(113, 156, 201, 0.24);
        border-radius: 1rem;
        padding: 1.5rem;
        box-shadow: 0 24px 60px rgba(4, 9, 16, 0.45);
      }
      .card h2 {
        margin: 0 0 0.75rem;
        font-size: 1.1rem;
        text-transform: uppercase;
        letter-spacing: 0.1em;
        color: #7aa6d9;
      }
      .badge {
        display: inline-flex;
        align-items: center;
        gap: 0.4rem;
        padding: 0.2rem 0.6rem;
        border-radius: 999px;
        font-size: 0.85rem;
        text-transform: uppercase;
        letter-spacing: 0.12em;
        font-weight: 600;
      }
      .badge.ok {
        background: rgba(52, 199, 89, 0.18);
        color: #83f4b1;
      }
      .badge.warn {
        background: rgba(255, 204, 0, 0.2);
        color: #f8e178;
      }
      .badge.err {
        background: rgba(255, 69, 58, 0.18);
        color: #ff998f;
      }
      .badge.critical {
        background: rgba(255, 99, 71, 0.24);
        color: #ffaea3;
      }
      table {
        width: 100%;
        border-collapse: collapse;
        border-radius: 1rem;
        overflow: hidden;
        background: rgba(12, 18, 28, 0.72);
        border: 1px solid rgba(112, 149, 192, 0.2);
      }
      thead {
        background: rgba(67, 103, 148, 0.35);
        text-transform: uppercase;
        letter-spacing: 0.08em;
        font-size: 0.75rem;
      }
      th,
      td {
        padding: 0.85rem 1rem;
        text-align: left;
      }
      tbody tr:nth-child(even) {
        background: rgba(13, 20, 30, 0.65);
      }
      tbody tr:hover {
        background: rgba(51, 84, 128, 0.25);
      }
      td.actions {
        display: flex;
        gap: 0.5rem;
        flex-wrap: wrap;
      }
      button.action {
        background: rgba(72, 122, 183, 0.18);
        color: #9cc7ff;
        border: 1px solid rgba(156, 199, 255, 0.25);
        border-radius: 999px;
        padding: 0.35rem 0.9rem;
        font-size: 0.85rem;
        cursor: pointer;
        transition: background 0.15s ease, transform 0.15s ease;
      }
      button.action:hover:not(:disabled) {
        background: rgba(122, 172, 233, 0.25);
        transform: translateY(-1px);
      }
      button.action:disabled {
        opacity: 0.45;
        cursor: not-allowed;
      }
      .message {
        padding: 0.85rem 1rem;
        border-radius: 0.75rem;
        border: 1px solid transparent;
        font-size: 0.9rem;
      }
      .message.hidden {
        display: none;
      }
      .message.success {
        border-color: rgba(68, 207, 124, 0.35);
        background: rgba(32, 163, 86, 0.18);
        color: #8bf2b5;
      }
      .message.error {
        border-color: rgba(255, 99, 71, 0.35);
        background: rgba(255, 99, 71, 0.1);
        color: #ffb3a6;
      }
      .footnote {
        font-size: 0.85rem;
        color: #6c8bad;
      }
      code {
        background: rgba(102, 140, 189, 0.2);
        padding: 0.2rem 0.4rem;
        border-radius: 0.4rem;
      }
      @media (max-width: 720px) {
        body {
          padding: 2rem 1rem 3rem;
        }
        .meta {
          flex-direction: column;
          align-items: flex-start;
        }
        td.actions {
          flex-direction: column;
        }
      }
    </style>
  </head>
  <body>
    <main class="layout">
      <header class="hero">
        <h1 id="app-name">Fenrir Control Plane</h1>
        <p class="subtitle">
          Operational Dashboard für den Ticketsystem-Monolithen – SSH, CLI und
          HTTP Services im Überblick.
        </p>
        <div class="meta">
          <span id="app-version">Version unbekannt</span>
          <span id="http-endpoint">HTTP Endpoint: n/a</span>
          <span id="updated-at">Aktualisiert: -</span>
        </div>
      </header>

      <section class="token-panel" id="token-panel">
        <label for="token-input">Control-Plane Token</label>
        <div class="token-input-group">
          <input
            id="token-input"
            type="password"
            placeholder="Bearer Token eingeben"
            autocomplete="off"
          />
          <button id="apply-token" class="action">Übernehmen</button>
          <button id="clear-token" class="action">Löschen</button>
        </div>
        <p id="token-status" class="footnote">Kein Token gesetzt – nur öffentliche Infos verfügbar.</p>
      </section>

      <section class="cards">
        <article class="card">
          <h2>Health Status</h2>
          <div class="badge" id="live-status">Live: -</div>
          <div class="badge" id="ready-status">Ready: -</div>
          <p class="footnote">
            Readiness berücksichtigt registrierte Dienste sowie Telemetrie-Probes.
          </p>
        </article>
        <article class="card">
          <h2>REST API</h2>
          <p>
            <code>GET /health/live</code><br />
            <code>GET /health/ready</code><br />
            <code>GET /metrics</code><br />
            <code>GET /services</code><br />
            <code>GET /scheduler/jobs</code>
          </p>
          <p class="footnote">
            Lifecycle-Aktionen: <code>POST /services/&lt;id&gt;/(start|stop|restart)</code>
            mit optionalem <code>{"force": true}</code>.
          </p>
        </article>
        <article class="card">
          <h2>CLI Steuerung</h2>
          <p>
            <code>services list</code><br />
            <code>services start &lt;id&gt;</code><br />
            <code>services stop &lt;id&gt; [--force]</code><br />
            <code>services restart &lt;id&gt; [--force]</code>
          </p>
          <p class="footnote">
            Kritische Services verlangen ein explizites Bestätigen mittels Force.
          </p>
        </article>
      </section>

      <section class="card">
        <h2>Service Registry</h2>
        <div id="action-message" class="message hidden"></div>
        <div class="footnote" style="margin-bottom: 0.75rem;">
          Aktionen werden live gegen den Service-Katalog ausgeführt. Kritische
          Einträge sind markiert.
        </div>
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>Typ</th>
              <th>Status</th>
              <th>Beschreibung</th>
              <th>Hinweis</th>
              <th>Aktionen</th>
            </tr>
          </thead>
          <tbody id="services-body">
            <tr>
              <td colspan="6">Lade aktuelle Serviceliste...</td>
            </tr>
          </tbody>
        </table>
      </section>

      <section class="card">
        <h2>Scheduler Jobs</h2>
        <div class="footnote" style="margin-bottom: 0.75rem;">
          Zeigt periodische Hintergrundaufgaben inkl. Intervall und Aktivitätsstatus.
        </div>
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>Intervall</th>
              <th>Beschreibung</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody id="jobs-body">
            <tr>
              <td colspan="4">Lade Scheduler-Jobs...</td>
            </tr>
          </tbody>
        </table>
      </section>

      <footer class="footnote">
        &copy; Fenrir Ticketsystem – Security-first Monolith mit modularen
        Transports (SSH, CLI, HTTP) und dynamischem Service-Runtime-Management.
      </footer>
    </main>

    <script>
      const API = {
        info: "/info",
        live: "/health/live",
        ready: "/health/ready",
        services: "/services",
        jobs: "/scheduler/jobs",
      };
      const TOKEN_STORAGE_KEY = "fenrir-control-token";

      const state = {
        messageEl: null,
        servicesBody: null,
        jobsBody: null,
        liveBadge: null,
        readyBadge: null,
        updatedEl: null,
        appNameEl: null,
        appVersionEl: null,
        endpointEl: null,
        tokenInput: null,
        tokenStatus: null,
        token: null,
      };

      async function fetchJson(url, options = {}) {
        const headers = new Headers(options.headers || {});
        headers.set("Accept", "application/json");
        if (state.token) {
          headers.set("Authorization", `Bearer ${state.token}`);
        }
        const response = await fetch(url, { ...options, headers });
        const text = await response.text();
        const data = text ? JSON.parse(text) : null;
        if (!response.ok) {
          const message = data?.message || data?.error || response.statusText;
          throw new Error(`${response.status} ${message}`);
        }
        return data;
      }

      function setBadge(el, value, okLabel, warnLabel) {
        if (!el) return;
        el.classList.remove("badge", "ok", "warn", "err");
        if (value === true) {
          el.classList.add("badge", "ok");
          el.textContent = okLabel;
        } else if (value === false) {
          el.classList.add("badge", "err");
          el.textContent = warnLabel;
        } else {
          el.classList.add("badge", "warn");
          el.textContent = "Unbekannt";
        }
      }

      function formatSince(seconds) {
        if (seconds == null) return "-";
        if (seconds < 60) return `${seconds}s`;
        const minutes = Math.floor(seconds / 60);
        if (minutes < 60) return `${minutes}m`;
        const hours = Math.floor(minutes / 60);
        if (hours < 24) return `${hours}h`;
        const days = Math.floor(hours / 24);
        return `${days}d`;
      }

      function setMessage(message, type = "success") {
        if (!state.messageEl) return;
        if (!message) {
          state.messageEl.classList.add("hidden");
          state.messageEl.textContent = "";
          return;
        }
        state.messageEl.className = `message ${type}`;
        state.messageEl.textContent = message;
        if (type === "success") {
          setTimeout(() => setMessage(""), 4000);
        }
      }

      function shouldDisable(action, status) {
        switch (action) {
          case "start":
            return status === "starting" || status === "active";
          case "stop":
            return status === "stopped" || status === "standby";
          case "restart":
            return status === "stopped" || status === "standby";
          default:
            return false;
        }
      }

      function createActionButton(label, action, id, service) {
        const button = document.createElement("button");
        button.className = "action";
        button.textContent = label;
        button.disabled = shouldDisable(action, service.status);
        button.addEventListener("click", async () => {
          try {
            const previous = button.disabled;
            button.disabled = true;
            await performAction(id, action, service.critical);
            await refreshServices();
            button.disabled = shouldDisable(action, service.status);
          } catch (err) {
            setMessage(err.message, "error");
            button.disabled = false;
          }
        });
        return button;
      }

      async function performAction(id, action, critical) {
        let force = false;
        if (critical && (action === "stop" || action === "restart")) {
          const confirmed = window.confirm(
            `Service ${id} ist kritisch. Aktion erzwingen?`
          );
          if (!confirmed) {
            throw new Error("Aktion vom Operator abgebrochen");
          }
          force = true;
        }

        const url = `/services/${encodeURIComponent(id)}/${action}`;
        const options = {
          method: "POST",
        };
        if (action !== "start") {
          options.headers = { "Content-Type": "application/json" };
          options.body = JSON.stringify({ force });
        }

        const data = await fetchJson(url, options);
        const service = data.service;
        const status = service?.status || "?";
        setMessage(
          `${data.action.toUpperCase()} ${id}: ${data.outcome.replace(/_/g, " ")} → Status ${status.toUpperCase()}`,
          "success"
        );
      }

      function renderServices(services) {
        if (!state.servicesBody) return;
        state.servicesBody.innerHTML = "";
        if (!services?.length) {
          const row = document.createElement("tr");
          const cell = document.createElement("td");
          cell.colSpan = 6;
          cell.textContent = "Keine Services registriert.";
          row.appendChild(cell);
          state.servicesBody.appendChild(row);
          return;
        }

        for (const service of services) {
          const row = document.createElement("tr");

          const idCell = document.createElement("td");
          idCell.textContent = service.id;
          if (service.critical) {
            const critical = document.createElement("span");
            critical.className = "badge critical";
            critical.textContent = "CRITICAL";
            critical.style.marginLeft = "0.5rem";
            idCell.appendChild(critical);
          }
          row.appendChild(idCell);

          const kindCell = document.createElement("td");
          kindCell.textContent = service.kind;
          row.appendChild(kindCell);

          const statusCell = document.createElement("td");
          statusCell.textContent = `${service.status} (${formatSince(service.since_seconds)})`;
          row.appendChild(statusCell);

          const descCell = document.createElement("td");
          descCell.textContent = service.description;
          row.appendChild(descCell);

          const noteCell = document.createElement("td");
          noteCell.textContent = service.note || "-";
          row.appendChild(noteCell);

          const actionCell = document.createElement("td");
          actionCell.className = "actions";
          actionCell.appendChild(createActionButton("Start", "start", service.id, service));
          actionCell.appendChild(createActionButton("Stop", "stop", service.id, service));
          actionCell.appendChild(createActionButton("Restart", "restart", service.id, service));
          row.appendChild(actionCell);

          state.servicesBody.appendChild(row);
        }
      }

      function renderJobs(jobs) {
        if (!state.jobsBody) return;
        state.jobsBody.innerHTML = "";
        if (!jobs?.length) {
          const row = document.createElement("tr");
          const cell = document.createElement("td");
          cell.colSpan = 4;
          cell.textContent = "Keine Scheduler-Jobs registriert.";
          row.appendChild(cell);
          state.jobsBody.appendChild(row);
          return;
        }

        for (const job of jobs) {
          const row = document.createElement("tr");

          const idCell = document.createElement("td");
          idCell.textContent = job.id;
          row.appendChild(idCell);

          const intervalCell = document.createElement("td");
          intervalCell.textContent = `${job.interval_seconds}s`;
          row.appendChild(intervalCell);

          const descCell = document.createElement("td");
          descCell.textContent = job.description;
          row.appendChild(descCell);

          const statusCell = document.createElement("td");
          statusCell.textContent = job.active ? "aktiv" : "inaktiv";
          row.appendChild(statusCell);

          state.jobsBody.appendChild(row);
        }
      }

      async function refreshServices() {
        try {
          const payload = await fetchJson(API.services);
          renderServices(payload.services);
        } catch (err) {
          setMessage(`Services konnten nicht geladen werden: ${err.message}`, "error");
        }
      }

      async function refreshJobs() {
        try {
          const payload = await fetchJson(API.jobs);
          renderJobs(payload.jobs);
        } catch (err) {
          setMessage(`Scheduler-Jobs konnten nicht geladen werden: ${err.message}`, "error");
        }
      }

      async function refreshHealth() {
        try {
          const [live, ready] = await Promise.all([
            fetchJson(API.live),
            fetchJson(API.ready),
          ]);
          setBadge(state.liveBadge, live?.live, "Live: OK", "Live: Fehler");
          setBadge(state.readyBadge, ready?.ready, "Ready: OK", "Ready: Fehler");
        } catch (err) {
          setBadge(state.liveBadge, null, "Live: ?", "Live: ?");
          setBadge(state.readyBadge, null, "Ready: ?", "Ready: ?");
          setMessage(`Health-Abfragen fehlgeschlagen: ${err.message}`, "error");
        }
      }

      async function refreshInfo() {
        try {
          const info = await fetchJson(API.info);
          if (state.appNameEl) {
            state.appNameEl.textContent = `${info.app.name} Control Plane`;
          }
          if (state.appVersionEl) {
            state.appVersionEl.textContent = `Version ${info.app.version}`;
          }
          if (state.endpointEl) {
            state.endpointEl.textContent = `HTTP Endpoint: ${info.http.base_url}`;
          }
        } catch (err) {
          setMessage(`Info-Endpunkt nicht erreichbar: ${err.message}`, "error");
        }
      }

      async function refreshAll() {
        await Promise.all([refreshInfo(), refreshHealth(), refreshServices(), refreshJobs()]);
        if (state.updatedEl) {
          const date = new Date();
          state.updatedEl.textContent = `Aktualisiert: ${date.toLocaleTimeString()}`;
        }
      }

      function updateTokenStatus() {
        if (!state.tokenStatus) return;
        if (state.token) {
          state.tokenStatus.textContent = "Token aktiv – autorisierte Daten werden geladen.";
        } else {
          state.tokenStatus.textContent = "Kein Token gesetzt – nur öffentliche Infos verfügbar.";
        }
      }

      function bindTokenPanel() {
        const apply = document.getElementById("apply-token");
        const clear = document.getElementById("clear-token");
        if (apply) {
          apply.addEventListener("click", () => {
            const token = state.tokenInput?.value.trim();
            if (!token) {
              setMessage("Bitte Token eingeben", "error");
              return;
            }
            state.token = token;
            window.localStorage.setItem(TOKEN_STORAGE_KEY, token);
            if (state.tokenInput) {
              state.tokenInput.value = "";
            }
            updateTokenStatus();
            setMessage("Token übernommen", "success");
            refreshAll();
          });
        }
        if (clear) {
          clear.addEventListener("click", () => {
            state.token = null;
            window.localStorage.removeItem(TOKEN_STORAGE_KEY);
            updateTokenStatus();
            setMessage("Token entfernt", "success");
            refreshAll();
          });
        }
      }

      async function initialise() {
        state.messageEl = document.getElementById("action-message");
        state.servicesBody = document.getElementById("services-body");
        state.jobsBody = document.getElementById("jobs-body");
        state.liveBadge = document.getElementById("live-status");
        state.readyBadge = document.getElementById("ready-status");
        state.updatedEl = document.getElementById("updated-at");
        state.appNameEl = document.getElementById("app-name");
        state.appVersionEl = document.getElementById("app-version");
        state.endpointEl = document.getElementById("http-endpoint");
        state.tokenInput = document.getElementById("token-input");
        state.tokenStatus = document.getElementById("token-status");
        const stored = window.localStorage.getItem(TOKEN_STORAGE_KEY);
        if (stored) {
          state.token = stored;
        }
        updateTokenStatus();
        bindTokenPanel();

        await refreshAll();
        setInterval(refreshHealth, 20000);
        setInterval(refreshServices, 15000);
        setInterval(refreshJobs, 20000);
      }

      if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", initialise);
      } else {
        initialise();
      }
    </script>
  </body>
</html>
"#;
