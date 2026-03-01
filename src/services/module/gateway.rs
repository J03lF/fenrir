use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, info, warn};
use uuid::Uuid;

const SERVICE_TOKEN_HEADER: &str = "x-fenrir-service-token";

use super::clients::ModuleClientSettings;
use super::types::ModuleIngressError;
use super::{ModuleIngressTarget, ModuleService};
use crate::domain::module::{ModuleId, ModuleRuntimeError};
use crate::services::ServiceDiagnostics;

#[derive(Clone)]
pub struct GatewaySettings {
    pub timeout: Duration,
    pub retries: u32,
    pub backoff: Duration,
}

impl From<&ModuleClientSettings> for GatewaySettings {
    fn from(settings: &ModuleClientSettings) -> Self {
        Self {
            timeout: settings.timeout,
            retries: settings.retries,
            backoff: settings.backoff,
        }
    }
}

pub struct RuntimeGatewayRegistry {
    entries: Mutex<HashMap<ModuleId, RuntimeGatewayHandle>>,
    settings: GatewaySettings,
    diagnostics: Arc<ServiceDiagnostics>,
}

impl RuntimeGatewayRegistry {
    pub fn new(settings: GatewaySettings, diagnostics: Arc<ServiceDiagnostics>) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            settings,
            diagnostics,
        }
    }

    pub async fn ensure(
        &self,
        host: Weak<ModuleService>,
        module_id: &ModuleId,
    ) -> Result<String, ModuleRuntimeError> {
        let mut guard = self.entries.lock().await;
        if let Some(handle) = guard.get(module_id) {
            if !handle.task.is_finished() {
                return Ok(handle.endpoint.clone());
            }
        }

        let handle = spawn_gateway_server(
            module_id.clone(),
            host,
            self.settings.clone(),
            Arc::clone(&self.diagnostics),
        )
        .await?;
        let endpoint = handle.endpoint.clone();
        guard.insert(module_id.clone(), handle);
        Ok(endpoint)
    }

    pub async fn stop(&self, module_id: &ModuleId) {
        let handle = {
            let mut guard = self.entries.lock().await;
            guard.remove(module_id)
        };
        if let Some(mut handle) = handle {
            if let Some(tx) = handle.shutdown.take() {
                let _ = tx.send(());
            }
            if !handle.task.is_finished() {
                if let Err(err) = handle.task.await {
                    debug!(
                        module = %module_id,
                        error = %err,
                        "runtime gateway task join failed"
                    );
                }
            }
        }
    }
}

struct RuntimeGatewayHandle {
    endpoint: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

async fn spawn_gateway_server(
    module_id: ModuleId,
    host: Weak<ModuleService>,
    settings: GatewaySettings,
    diagnostics: Arc<ServiceDiagnostics>,
) -> Result<RuntimeGatewayHandle, ModuleRuntimeError> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|err| {
        ModuleRuntimeError::StartFailed {
            module_id: module_id.to_string(),
            reason: format!("failed to bind runtime gateway socket: {err}"),
        }
    })?;
    let addr = listener
        .local_addr()
        .map_err(|err| ModuleRuntimeError::StartFailed {
            module_id: module_id.to_string(),
            reason: format!("failed to read runtime gateway socket: {err}"),
        })?;
    let endpoint = format!("http://127.0.0.1:{}/call", addr.port());
    let context = RuntimeGatewayContext::new(module_id.clone(), host, settings, diagnostics);
    let router = Router::new()
        .route("/call", post(gateway_call))
        .with_state(context);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let module_id_for_task = module_id.clone();
    let task = tokio::spawn(async move {
        let server =
            axum::serve(listener, router.into_make_service()).with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });
        if let Err(err) = server.await {
            warn!(
                module = %module_id_for_task,
                error = %err,
                "runtime gateway exited with error"
            );
        }
    });
    info!(
        module = %module_id,
        endpoint = %endpoint,
        "runtime gateway started"
    );
    Ok(RuntimeGatewayHandle {
        endpoint,
        shutdown: Some(shutdown_tx),
        task,
    })
}

#[derive(Clone)]
struct RuntimeGatewayContext {
    module_id: ModuleId,
    host: Weak<ModuleService>,
    client: Client,
    retries: u32,
    backoff: Duration,
    diagnostics: Arc<ServiceDiagnostics>,
}

impl RuntimeGatewayContext {
    fn new(
        module_id: ModuleId,
        host: Weak<ModuleService>,
        settings: GatewaySettings,
        diagnostics: Arc<ServiceDiagnostics>,
    ) -> Self {
        let client = Client::builder()
            .timeout(settings.timeout)
            .build()
            .expect("gateway client must build");
        Self {
            module_id,
            host,
            client,
            retries: settings.retries,
            backoff: settings.backoff,
            diagnostics,
        }
    }

    fn metric_name(&self) -> String {
        format!("module-runtime-gateway/{}", self.module_id)
    }

    fn record_probe(&self, latency: Duration, success: bool) {
        let latency_ms = latency.as_secs_f64() * 1000.0;
        self.diagnostics
            .record_probe(&self.metric_name(), latency_ms, success);
    }

    async fn execute(
        &self,
        payload: GatewayRequest,
        audit_id: &str,
    ) -> Result<GatewayResponsePayload, GatewayError> {
        let host = self.host.upgrade().ok_or(GatewayError::ModuleUnavailable)?;
        let target_id = normalize_target(&payload.target)?;
        let ingress = host
            .resolve_ingress_target(target_id)
            .await
            .map_err(GatewayError::Ingress)?;
        let url = build_target_url(&ingress, &payload.path, &payload.query)?;
        let method = parse_method(payload.verb.as_deref())?;

        let token = host
            .active_service_token_value(&self.module_id)
            .await
            .ok_or(GatewayError::MissingToken)?;

        let mut attempt = 0;
        loop {
            match self
                .send_request(
                    &method,
                    &url,
                    &token,
                    &payload.headers,
                    payload.body.as_ref(),
                    payload.timeout_ms,
                )
                .await
            {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let headers = map_headers(response.headers());
                    let bytes = response.bytes().await.map_err(GatewayError::Http)?;
                    let body = GatewayBody::from_bytes(&bytes);
                    return Ok(GatewayResponsePayload {
                        status,
                        audit_id: audit_id.to_string(),
                        headers,
                        body,
                    });
                }
                Err(err) if should_retry(&err, attempt, self.retries) => {
                    attempt += 1;
                    sleep(self.backoff).await;
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn send_request(
        &self,
        method: &Method,
        url: &str,
        token: &str,
        headers: &HashMap<String, String>,
        body: Option<&serde_json::Value>,
        timeout_override_ms: Option<u64>,
    ) -> Result<reqwest::Response, GatewayError> {
        let mut request = self.client.request(method.clone(), url);
        if let Some(timeout) = timeout_override_ms {
            request = request.timeout(Duration::from_millis(timeout));
        }
        let has_user_service_token = headers.contains_key(SERVICE_TOKEN_HEADER);
        let mut has_content_type = false;
        for (key, value) in headers {
            let name = HeaderName::try_from(key.as_str())
                .map_err(|_| GatewayError::InvalidHeader(key.clone()))?;
            if name == CONTENT_TYPE {
                has_content_type = true;
            }
            let header_value = HeaderValue::try_from(value.as_str())
                .map_err(|_| GatewayError::InvalidHeader(key.clone()))?;
            request = request.header(name, header_value);
        }
        if !has_user_service_token {
            request = request.header(SERVICE_TOKEN_HEADER, format!("Bearer {token}"));
        }
        if let Some(body) = body {
            let serialized = serde_json::to_vec(body)
                .map_err(|err| GatewayError::Serialization(err.to_string()))?;
            if !has_content_type {
                request =
                    request.header(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            }
            request = request.body(serialized);
        }
        request.send().await.map_err(GatewayError::Http)
    }
}

async fn gateway_call(
    State(ctx): State<RuntimeGatewayContext>,
    Json(payload): Json<GatewayRequest>,
) -> Result<Json<GatewayResponsePayload>, (StatusCode, Json<GatewayErrorResponse>)> {
    let audit_id = Uuid::new_v4().to_string();
    let started = Instant::now();
    match ctx.execute(payload, &audit_id).await {
        Ok(response) => {
            ctx.record_probe(started.elapsed(), true);
            Ok(Json(response))
        }
        Err(err) => {
            ctx.record_probe(started.elapsed(), false);
            let status = err.status();
            let error_response = GatewayErrorResponse {
                audit_id,
                status: status.as_u16(),
                error: err.message(),
            };
            Err((status, Json(error_response)))
        }
    }
}

#[derive(Debug, Deserialize)]
struct GatewayRequest {
    target: String,
    #[serde(default)]
    verb: Option<String>,
    #[serde(default = "GatewayRequest::default_path")]
    path: String,
    #[serde(default)]
    body: Option<serde_json::Value>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    query: HashMap<String, String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

impl GatewayRequest {
    fn default_path() -> String {
        "/".to_string()
    }
}

#[derive(Serialize)]
pub struct GatewayResponsePayload {
    pub status: u16,
    pub audit_id: String,
    pub headers: HashMap<String, String>,
    pub body: GatewayBody,
}

#[derive(Serialize)]
#[serde(tag = "format", content = "value")]
pub enum GatewayBody {
    Empty,
    Json(serde_json::Value),
    Text(String),
    Base64(String),
}

impl GatewayBody {
    fn from_bytes(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return GatewayBody::Empty;
        }
        if let Ok(json) = serde_json::from_slice(bytes) {
            return GatewayBody::Json(json);
        }
        if let Ok(text) = std::str::from_utf8(bytes) {
            return GatewayBody::Text(text.to_string());
        }
        GatewayBody::Base64(BASE64_STANDARD.encode(bytes))
    }
}

#[derive(Serialize)]
pub struct GatewayErrorResponse {
    pub audit_id: String,
    pub status: u16,
    pub error: String,
}

#[derive(Debug)]
enum GatewayError {
    ModuleUnavailable,
    InvalidTarget(String),
    MissingToken,
    InvalidHeader(String),
    Serialization(String),
    InvalidMethod(String),
    Ingress(ModuleIngressError),
    Http(reqwest::Error),
}

impl GatewayError {
    fn status(&self) -> StatusCode {
        match self {
            GatewayError::ModuleUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            GatewayError::InvalidTarget(_)
            | GatewayError::InvalidHeader(_)
            | GatewayError::InvalidMethod(_) => StatusCode::BAD_REQUEST,
            GatewayError::MissingToken => StatusCode::SERVICE_UNAVAILABLE,
            GatewayError::Serialization(_) => StatusCode::INTERNAL_SERVER_ERROR,
            GatewayError::Ingress(_) => StatusCode::BAD_GATEWAY,
            GatewayError::Http(err) => {
                if err.is_timeout() {
                    StatusCode::GATEWAY_TIMEOUT
                } else {
                    StatusCode::BAD_GATEWAY
                }
            }
        }
    }

    fn message(&self) -> String {
        match self {
            GatewayError::ModuleUnavailable => "module service unavailable".to_string(),
            GatewayError::InvalidTarget(target) => {
                format!("invalid gateway target '{target}'")
            }
            GatewayError::MissingToken => "no active service token available".to_string(),
            GatewayError::InvalidHeader(header) => {
                format!("invalid header '{header}'")
            }
            GatewayError::Serialization(err) => err.to_string(),
            GatewayError::InvalidMethod(method) => {
                format!("unsupported verb '{method}'")
            }
            GatewayError::Ingress(err) => err.to_string(),
            GatewayError::Http(err) => err.to_string(),
        }
    }
}

fn normalize_target(raw: &str) -> Result<&str, GatewayError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(GatewayError::InvalidTarget(raw.to_string()));
    }
    Ok(trimmed.strip_prefix("service://").unwrap_or(trimmed))
}

fn parse_method(value: Option<&str>) -> Result<Method, GatewayError> {
    match value {
        Some(method) => Method::from_bytes(method.trim().to_uppercase().as_bytes())
            .map_err(|_| GatewayError::InvalidMethod(method.to_string())),
        None => Ok(Method::POST),
    }
}

fn build_target_url(
    target: &ModuleIngressTarget,
    path: &str,
    query: &HashMap<String, String>,
) -> Result<String, GatewayError> {
    let base = match target {
        ModuleIngressTarget::RuntimePort { port, .. } => format!("http://127.0.0.1:{port}"),
        ModuleIngressTarget::DevService { endpoint, .. }
        | ModuleIngressTarget::DeclaredService { endpoint, .. } => format!("http://{endpoint}"),
    };
    let normalized_path = normalize_path(path);
    let query_suffix = encode_query(query);
    Ok(format!("{base}{normalized_path}{query_suffix}"))
}

fn encode_query(params: &HashMap<String, String>) -> String {
    if params.is_empty() {
        return String::new();
    }
    let mut encoded = Vec::new();
    for (key, value) in params {
        encoded.push(format!(
            "{}={}",
            urlencoding::encode(key),
            urlencoding::encode(value)
        ));
    }
    format!("?{}", encoded.join("&"))
}

fn should_retry(err: &GatewayError, attempt: u32, max_retries: u32) -> bool {
    if attempt >= max_retries {
        return false;
    }
    matches!(
        err,
        GatewayError::Http(e) if e.is_timeout() || e.is_connect()
    )
}

fn map_headers(headers: &HeaderMap) -> HashMap<String, String> {
    let mut mapped = HashMap::new();
    for (key, value) in headers.iter() {
        if let Ok(text) = value.to_str() {
            mapped.insert(key.to_string(), text.to_string());
        }
    }
    mapped
}

fn normalize_path(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "/".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed)
    }
}
