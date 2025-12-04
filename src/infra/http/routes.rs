use std::collections::HashMap;
use std::convert::Infallible;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::extract::{OriginalUri, Path as AxumPath, Query, State};
use axum::http::{
    header::{HeaderName as AxumHeaderName, AUTHORIZATION, HOST},
    HeaderMap, HeaderValue, Method, StatusCode, Uri,
};
use axum::response::{
    sse::{Event, KeepAlive, Sse},
    Html, IntoResponse, Response,
};
use axum::routing::{any, delete, get, post};
use axum::Json;
use axum::Router;
use http_body_util::BodyExt;
use reqwest::{
    header::{HeaderName as ReqwestHeaderName, HeaderValue as ReqwestHeaderValue},
    Method as ReqwestMethod,
};
use serde::{Deserialize, Serialize};
use serde_json;
use time::Duration as TimeDuration;
use time::OffsetDateTime;
use tokio_stream::{
    wrappers::{errors::BroadcastStreamRecvError, BroadcastStream},
    StreamExt,
};
use tracing::warn;
use urlencoding::decode;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::domain::module::{
    InstalledModule, ModuleError, ModuleId, ModuleInstallResult, ModuleInstallSource,
    ModuleInstallStatus, ModuleManifest, ModuleRegistryError, ModuleRuntimeError,
    ModuleSearchQuery, ModuleServiceError, ModuleStorageError, ModuleVersion,
};
use crate::infra::{logging, telemetry};
use crate::security::auth::{AuthError, ControlPlaneAuthorizer, Role};
use crate::security::identity::{IdentityProvider, IssueTokenRequest};
use crate::security::manager::SecurityError;
use crate::security::service::ServiceScope;
use crate::security::service_tokens::{DelegatedActor, DelegatedTokenClaims, ServiceTokenError};
use crate::services::scheduler::ScheduledJobSnapshot;
use crate::services::{
    module::{ModuleIngressError, ModuleIngressTarget},
    AppServices, ServiceActionKind, ServiceControlError, ServiceIngressAccess,
    ServiceIngressMetadata, ServiceIngressProtocol, ServiceRateLimit, ServiceRegistry,
    ServiceSecurityMetadata, ServiceSnapshot,
};
use crate::utils::messages::infra::http::{self as http_messages, ProblemText};
use crate::utils::{
    format_offset_datetime, format_optional_offset_datetime, system_time_to_rfc3339,
};
use fenrir_module_kit::{ModuleTokenExchangeRequest, ModuleTokenExchangeResponse};

use super::gateway::{HTTP_GATEWAY_CLIENT, SERVICE_RATE_LIMITER};
use super::state::{HttpInfo, HttpState};

const SERVICE_RESOURCE_STALE_AFTER_SECS: i64 = 60;
const ALLOWED_MODULE_TOKEN_SCOPES: &[&str] = &["db:write"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum GatewayRequestKind {
    Http,
    Grpc,
}

impl GatewayRequestKind {
    fn as_str(self) -> &'static str {
        match self {
            GatewayRequestKind::Http => "http",
            GatewayRequestKind::Grpc => "grpc",
        }
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
    id: String,
    name: String,
    kind: &'static str,
    status: &'static str,
    note: Option<String>,
    since_seconds: Option<u64>,
    critical: bool,
    tags: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    security: Option<ServiceSecurityView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ingress: Option<ServiceIngressView>,
}

#[derive(Clone, Serialize)]
struct ServiceSummary {
    id: String,
    name: String,
    kind: &'static str,
    status: &'static str,
    since_seconds: Option<u64>,
    description: String,
    note: Option<String>,
    critical: bool,
    tags: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    security: Option<ServiceSecurityView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ingress: Option<ServiceIngressView>,
}

#[derive(Clone, Serialize)]
struct ServiceSecurityView {
    internal_only: bool,
    allowed_roles: Vec<&'static str>,
    required_scopes: Vec<String>,
    tenant_mode: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tenant_values: Vec<String>,
}

#[derive(Clone, Serialize)]
struct ServiceIngressView {
    access: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    health_endpoint: Option<String>,
    protocols: Vec<&'static str>,
    rate_limit: ServiceIngressRateLimitView,
}

#[derive(Clone, Serialize)]
struct ServiceIngressRateLimitView {
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit_per_second: Option<u32>,
}

#[derive(Serialize)]
struct MetricsResponse {
    uptime_seconds: Option<u64>,
    live: bool,
    ready: bool,
    counters: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    service_resources: Vec<ServiceResourceView>,
}

#[derive(Serialize)]
struct ServiceResourceView {
    id: String,
    cpu_percent: Option<f32>,
    memory_bytes: Option<u64>,
    memory_peak_bytes: Option<u64>,
    updated_at: Option<String>,
    stale: bool,
    reported: bool,
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
struct IdentityUsersResponse {
    users: Vec<IdentityUserView>,
}

#[derive(Serialize)]
struct IdentityUserView {
    user_id: String,
    display_name: Option<String>,
    role: String,
    created_at: String,
    last_issued_at: Option<String>,
    token_count: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tokens: Vec<IdentityUserTokenView>,
}

#[derive(Serialize)]
struct IdentityUserTokenView {
    token_id: String,
    fingerprint: String,
    issued_at: String,
    expires_at: String,
    key_id: String,
}

#[derive(Deserialize)]
struct IssueIdentityTokenPayload {
    user_id: String,
    role: Option<String>,
    display_name: Option<String>,
}

#[derive(Serialize)]
struct IssueIdentityTokenResponse {
    token: String,
    token_id: String,
    user_id: String,
    role: String,
    expires_at: String,
    fingerprint: String,
    issued_at: String,
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
struct ReleaseDevOverridesResponse {
    released: Vec<String>,
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
    source: ModuleInstallSource,
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
    source: ModuleInstallSource,
}

#[derive(Serialize)]
struct ModuleUpdateResponse {
    results: Vec<ModuleInstallResponse>,
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
        self.kind().required_role()
    }

    fn execute(
        &self,
        services: &AppServices,
        force: bool,
    ) -> Vec<crate::services::ServiceActionReport> {
        match self.kind() {
            ServiceActionKind::Start => services.start_all_non_core(),
            ServiceActionKind::Stop => services.stop_all_non_core(force),
            ServiceActionKind::Restart => services.restart_all_non_core(force),
        }
    }

    fn kind(&self) -> ServiceActionKind {
        match self {
            BulkServiceActionKind::Start => ServiceActionKind::Start,
            BulkServiceActionKind::Stop => ServiceActionKind::Stop,
            BulkServiceActionKind::Restart => ServiceActionKind::Restart,
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
        id: svc.descriptor.id.clone(),
        name: svc.descriptor.name.clone(),
        kind: svc.descriptor.kind.as_str(),
        status: svc.status.label(),
        since_seconds: svc.since.elapsed().ok().map(|duration| duration.as_secs()),
        description: svc.descriptor.description.clone(),
        note: svc.note.clone(),
        critical: svc.descriptor.critical,
        tags: svc.descriptor.tags.iter().map(|tag| tag.as_str()).collect(),
        security: svc.descriptor.security.as_ref().map(security_view),
        ingress: svc.descriptor.ingress.as_ref().map(ingress_view),
    }
}

fn is_module_placeholder(id: &str) -> bool {
    id.starts_with("module:") && !id.contains("::")
}

fn snapshot_to_state_event(snapshot: ServiceSnapshot) -> ServiceStateEvent {
    ServiceStateEvent {
        id: snapshot.descriptor.id.clone(),
        name: snapshot.descriptor.name.clone(),
        kind: snapshot.descriptor.kind.as_str(),
        status: snapshot.status.label(),
        note: snapshot.note.clone(),
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
        security: snapshot.descriptor.security.as_ref().map(security_view),
        ingress: snapshot.descriptor.ingress.as_ref().map(ingress_view),
    }
}

fn security_view(metadata: &ServiceSecurityMetadata) -> ServiceSecurityView {
    ServiceSecurityView {
        internal_only: metadata.internal_only,
        allowed_roles: metadata
            .allowed_roles()
            .iter()
            .map(|role| role.as_str())
            .collect(),
        required_scopes: metadata
            .required_scopes()
            .iter()
            .map(|scope| scope.as_str().to_string())
            .collect(),
        tenant_mode: metadata.tenant.mode_label(),
        tenant_values: metadata.tenant.values(),
    }
}

fn ingress_view(metadata: &ServiceIngressMetadata) -> ServiceIngressView {
    ServiceIngressView {
        access: metadata.access.as_str(),
        route_prefix: metadata.route_prefix.clone(),
        health_endpoint: metadata.health_endpoint.clone(),
        protocols: metadata
            .protocols
            .iter()
            .map(|protocol| protocol.as_str())
            .collect(),
        rate_limit: match metadata.rate_limit {
            ServiceRateLimit::Default => ServiceIngressRateLimitView {
                mode: "default",
                limit_per_second: None,
            },
            ServiceRateLimit::Unlimited => ServiceIngressRateLimitView {
                mode: "unlimited",
                limit_per_second: None,
            },
            ServiceRateLimit::CustomPerSecond(limit) => ServiceIngressRateLimitView {
                mode: "custom",
                limit_per_second: Some(limit),
            },
        },
    }
}

fn http_problem(status: StatusCode, text: ProblemText) -> ServiceActionProblem {
    ServiceActionProblem::new(status, text.code, text.message)
}

fn module_service_unavailable() -> ServiceActionProblem {
    http_problem(
        StatusCode::SERVICE_UNAVAILABLE,
        http_messages::problems::module_service_unavailable(),
    )
}

fn identity_service_unavailable() -> ServiceActionProblem {
    http_problem(
        StatusCode::SERVICE_UNAVAILABLE,
        http_messages::problems::identity_service_unavailable(),
    )
}

fn viewer_role_required() -> ServiceActionProblem {
    http_problem(
        StatusCode::FORBIDDEN,
        http_messages::problems::role_insufficient_viewer(),
    )
}

fn admin_role_required() -> ServiceActionProblem {
    http_problem(
        StatusCode::FORBIDDEN,
        http_messages::problems::role_insufficient_admin(),
    )
}

fn authorize_identity(
    state: &HttpState,
    headers: &HeaderMap,
    required: Role,
) -> Result<Arc<dyn IdentityProvider>, ServiceActionProblem> {
    authorize(&state.auth, &state.services, headers, required)?;
    state
        .services
        .identity()
        .ok_or_else(identity_service_unavailable)
}

fn parse_role_string(value: &str) -> Result<Role, ServiceActionProblem> {
    Role::from_str(value).map_err(|err| {
        http_problem(
            StatusCode::BAD_REQUEST,
            http_messages::problems::invalid_role(err.value()),
        )
    })
}

fn installed_module_to_view(installed: InstalledModule) -> InstalledModuleView {
    InstalledModuleView {
        manifest: installed.manifest,
        installed_at: system_time_to_rfc3339(installed.installed_at),
        path: installed.path,
        source: installed.source,
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
        source: result.source,
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
                format!("Module '{}' was not found", module),
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

fn module_runtime_problem(err: ModuleRuntimeError) -> ServiceActionProblem {
    let (status, code) = match err {
        ModuleRuntimeError::NotInstalled { .. } => (StatusCode::NOT_FOUND, "module_not_installed"),
        ModuleRuntimeError::AlreadyRunning { .. } => {
            (StatusCode::CONFLICT, "module_already_running")
        }
        ModuleRuntimeError::NotRunning { .. } => (StatusCode::CONFLICT, "module_not_running"),
        ModuleRuntimeError::StartFailed { .. } => {
            (StatusCode::INTERNAL_SERVER_ERROR, "module_start_failed")
        }
        ModuleRuntimeError::StopFailed { .. } => {
            (StatusCode::INTERNAL_SERVER_ERROR, "module_stop_failed")
        }
        ModuleRuntimeError::PortInUse { .. } => (StatusCode::CONFLICT, "module_port_in_use"),
        ModuleRuntimeError::NoAvailablePorts { .. } => {
            (StatusCode::SERVICE_UNAVAILABLE, "module_no_ports")
        }
        ModuleRuntimeError::InvalidState(_) => (StatusCode::CONFLICT, "module_invalid_state"),
        ModuleRuntimeError::Io(_) => (StatusCode::INTERNAL_SERVER_ERROR, "module_runtime_io"),
        ModuleRuntimeError::Quarantined { .. } => (StatusCode::CONFLICT, "module_quarantined"),
    };
    ServiceActionProblem::new(status, code, err.to_string())
}

fn module_validation_problem(code: &'static str, err: ModuleError) -> ServiceActionProblem {
    match err {
        ModuleError::Validation(msg) => {
            ServiceActionProblem::new(StatusCode::BAD_REQUEST, code, msg)
        }
    }
}

fn module_id_from_claims(claims: &DelegatedTokenClaims) -> Result<ModuleId, ServiceActionProblem> {
    match &claims.actor {
        DelegatedActor::Service { service_id, .. } => {
            let Some(raw_id) = service_id.strip_prefix("module:") else {
                return Err(http_problem(
                    StatusCode::FORBIDDEN,
                    http_messages::problems::module_token_invalid_service(service_id),
                ));
            };
            ModuleId::new(raw_id).map_err(|_| {
                http_problem(
                    StatusCode::BAD_REQUEST,
                    http_messages::problems::module_token_invalid_service(service_id),
                )
            })
        }
        _ => Err(http_problem(
            StatusCode::FORBIDDEN,
            http_messages::problems::module_token_service_only(),
        )),
    }
}

fn parse_requested_scopes(scopes: &[String]) -> Result<Vec<ServiceScope>, ServiceActionProblem> {
    if scopes.is_empty() {
        return Err(http_problem(
            StatusCode::BAD_REQUEST,
            http_messages::problems::module_token_scope_missing(),
        ));
    }
    let mut parsed = Vec::with_capacity(scopes.len());
    for scope in scopes {
        let scope_trimmed = scope.trim();
        if !ALLOWED_MODULE_TOKEN_SCOPES.contains(&scope_trimmed) {
            return Err(http_problem(
                StatusCode::BAD_REQUEST,
                http_messages::problems::module_token_scope_invalid(scope_trimmed),
            ));
        }
        let parsed_scope = ServiceScope::new(scope_trimmed).map_err(|_| {
            http_problem(
                StatusCode::BAD_REQUEST,
                http_messages::problems::module_token_scope_invalid(scope_trimmed),
            )
        })?;
        if !parsed.iter().any(|existing| existing == &parsed_scope) {
            parsed.push(parsed_scope);
        }
    }
    Ok(parsed)
}

fn token_expires_in_seconds(claims: &DelegatedTokenClaims) -> u64 {
    let now = OffsetDateTime::now_utc();
    if claims.expires_at <= now {
        return 0;
    }
    (claims.expires_at - now).whole_seconds().max(0) as u64
}

fn decode_service_id(value: &str) -> Result<String, ServiceActionProblem> {
    decode(value)
        .map(|decoded| decoded.into_owned())
        .map_err(|_| {
            http_problem(
                StatusCode::BAD_REQUEST,
                http_messages::problems::gateway_invalid_service(value),
            )
        })
}

fn extract_gateway_token(headers: &HeaderMap) -> Result<&str, ServiceActionProblem> {
    match extract_optional_gateway_token(headers)? {
        Some(token) => Ok(token),
        None => Err(http_problem(
            StatusCode::UNAUTHORIZED,
            http_messages::problems::gateway_token_missing(),
        )),
    }
}

fn extract_optional_gateway_token<'a>(
    headers: &'a HeaderMap,
) -> Result<Option<&'a str>, ServiceActionProblem> {
    let Some(value) = headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    let text = value
        .to_str()
        .map_err(|_| {
            http_problem(
                StatusCode::UNAUTHORIZED,
                http_messages::problems::gateway_token_invalid(),
            )
        })?
        .trim();
    if text.is_empty() {
        return Ok(None);
    }
    let Some(token) = text
        .strip_prefix("Bearer ")
        .or_else(|| text.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
    else {
        return Err(http_problem(
            StatusCode::UNAUTHORIZED,
            http_messages::problems::gateway_token_invalid(),
        ));
    };
    Ok(Some(token))
}

fn map_service_token_error(err: SecurityError) -> ServiceActionProblem {
    match err {
        SecurityError::ServiceToken(ServiceTokenError::NotFound)
        | SecurityError::ServiceToken(ServiceTokenError::Expired)
        | SecurityError::ServiceToken(ServiceTokenError::IdleTimeout) => http_problem(
            StatusCode::UNAUTHORIZED,
            http_messages::problems::gateway_token_expired(),
        ),
        other => http_problem(
            StatusCode::BAD_GATEWAY,
            http_messages::problems::gateway_security_failure(other),
        ),
    }
}

fn map_ingress_error(err: ModuleIngressError, service_id: &str) -> ServiceActionProblem {
    match err {
        ModuleIngressError::UnsupportedService(_) => http_problem(
            StatusCode::NOT_FOUND,
            http_messages::problems::gateway_unknown_service(service_id),
        ),
        ModuleIngressError::InvalidModuleId(_) => http_problem(
            StatusCode::BAD_REQUEST,
            http_messages::problems::gateway_invalid_service(service_id),
        ),
        ModuleIngressError::ModuleNotRunning(_) | ModuleIngressError::ModulePortUnknown(_) => {
            http_problem(
                StatusCode::SERVICE_UNAVAILABLE,
                http_messages::problems::gateway_service_unavailable(service_id),
            )
        }
        ModuleIngressError::DevServiceInactive { .. }
        | ModuleIngressError::DeclaredServiceMissing { .. } => http_problem(
            StatusCode::NOT_FOUND,
            http_messages::problems::gateway_unknown_service(service_id),
        ),
        ModuleIngressError::Runtime(inner) => http_problem(
            StatusCode::BAD_GATEWAY,
            http_messages::problems::gateway_service_failure(service_id, inner),
        ),
    }
}

fn service_route_path(
    ingress: Option<&ServiceIngressMetadata>,
    path: &str,
    query: Option<&str>,
) -> String {
    let tail = path.trim_start_matches('/');
    let mut buffer = String::new();
    if let Some(meta) = ingress {
        if let Some(prefix) = meta.route_prefix.as_ref() {
            if prefix == "/" {
                buffer.push('/');
                if !tail.is_empty() {
                    buffer.push_str(tail);
                }
            } else {
                let normalized = prefix
                    .trim_end_matches('/')
                    .trim_start_matches('/')
                    .to_string();
                if normalized.is_empty() {
                    buffer.push('/');
                } else {
                    buffer.push('/');
                    buffer.push_str(&normalized);
                }
                if !tail.is_empty() {
                    if !buffer.ends_with('/') {
                        buffer.push('/');
                    }
                    buffer.push_str(tail);
                }
            }
        } else {
            buffer.push('/');
            if !tail.is_empty() {
                buffer.push_str(tail);
            }
        }
    } else {
        buffer.push('/');
        if !tail.is_empty() {
            buffer.push_str(tail);
        }
    }
    if buffer.is_empty() {
        buffer.push('/');
    }
    if let Some(query) = query {
        if !query.is_empty() {
            buffer.push('?');
            buffer.push_str(query);
        }
    }
    buffer
}

fn build_target_url(target: &ModuleIngressTarget, path: &str) -> String {
    format!("http://{}{}", target_origin(target), path)
}

fn target_origin(target: &ModuleIngressTarget) -> String {
    match target {
        ModuleIngressTarget::RuntimePort { port, .. } => format!("127.0.0.1:{port}"),
        ModuleIngressTarget::DevService { endpoint, .. }
        | ModuleIngressTarget::DeclaredService { endpoint, .. } => endpoint.to_string(),
    }
}

fn target_log_label(target: &ModuleIngressTarget) -> String {
    match target {
        ModuleIngressTarget::RuntimePort { module_id, port } => {
            format!("module:{}@{}", module_id, port)
        }
        ModuleIngressTarget::DevService {
            module_id,
            service_id,
            endpoint,
        }
        | ModuleIngressTarget::DeclaredService {
            module_id,
            service_id,
            endpoint,
        } => format!("{} ({module_id} -> {endpoint})", service_id),
    }
}

fn map_reqwest_method(method: &Method) -> Result<ReqwestMethod, ServiceActionProblem> {
    ReqwestMethod::from_bytes(method.as_str().as_bytes()).map_err(|_| {
        http_problem(
            StatusCode::METHOD_NOT_ALLOWED,
            http_messages::problems::gateway_method_not_allowed(method.as_str()),
        )
    })
}

async fn collect_request_body(body: Body) -> Result<Vec<u8>, ServiceActionProblem> {
    body.collect()
        .await
        .map(|collected| collected.to_bytes().to_vec())
        .map_err(|err| {
            http_problem(
                StatusCode::BAD_REQUEST,
                http_messages::problems::gateway_body_read_failed(err),
            )
        })
}

async fn convert_upstream_response(
    response: reqwest::Response,
) -> Result<Response, ServiceActionProblem> {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = response.headers().clone();
    let body = response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|err| {
            http_problem(
                StatusCode::BAD_GATEWAY,
                http_messages::problems::gateway_upstream_read_failed(err),
            )
        })?;
    let mut builder = Response::builder().status(status);
    {
        let headers_mut = builder.headers_mut().expect("response headers available");
        for (name, value) in headers.iter() {
            if let (Ok(header_name), Ok(header_value)) = (
                AxumHeaderName::from_bytes(name.as_str().as_bytes()),
                HeaderValue::from_bytes(value.as_bytes()),
            ) {
                headers_mut.insert(header_name, header_value);
            }
        }
    }
    builder.body(Body::from(body)).map_err(|err| {
        http_problem(
            StatusCode::BAD_GATEWAY,
            http_messages::problems::gateway_upstream_conversion_failed(err),
        )
    })
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

async fn issue_module_service_token(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(payload): Json<ModuleTokenExchangeRequest>,
) -> Response {
    let Some(module_service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };
    let Some(security) = state.services.security_manager() else {
        return http_problem(
            StatusCode::SERVICE_UNAVAILABLE,
            http_messages::problems::gateway_security_unavailable(),
        )
        .into_response();
    };
    let token = match extract_gateway_token(&headers) {
        Ok(token) => token,
        Err(problem) => return problem.into_response(),
    };
    let claims = match security.validate_service_token(token) {
        Ok(claims) => claims,
        Err(err) => return map_service_token_error(err).into_response(),
    };
    let module_id = match module_id_from_claims(&claims) {
        Ok(id) => id,
        Err(problem) => return problem.into_response(),
    };
    let scopes = match parse_requested_scopes(&payload.scopes) {
        Ok(scopes) => scopes,
        Err(problem) => return problem.into_response(),
    };
    let issued = match module_service.issue_scoped_service_token(&module_id, scopes) {
        Ok(token) => token,
        Err(err) => {
            return http_problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                http_messages::problems::module_token_issue_failed(&err),
            )
            .into_response()
        }
    };
    let response = ModuleTokenExchangeResponse {
        token: issued.token,
        scopes: issued
            .claims
            .scopes
            .iter()
            .map(|scope| scope.as_str().to_string())
            .collect(),
        expires_in_seconds: token_expires_in_seconds(&issued.claims),
    };
    (StatusCode::OK, Json(response)).into_response()
}

async fn start_module_runtime(
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
    match service.start_module(&module_id).await {
        Ok(info) => (StatusCode::OK, Json(info)).into_response(),
        Err(err) => module_runtime_problem(err).into_response(),
    }
}

async fn stop_module_runtime(
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
    match service.stop(&module_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => module_runtime_problem(err).into_response(),
    }
}

async fn stop_all_module_runtimes(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Operator) {
        return problem.into_response();
    }
    let Some(service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };
    match service.stop_all_modules().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => module_runtime_problem(err).into_response(),
    }
}

async fn restart_module_runtime(
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
    match service.restart(&module_id).await {
        Ok(info) => (StatusCode::OK, Json(info)).into_response(),
        Err(err) => module_runtime_problem(err).into_response(),
    }
}

async fn release_dev_overrides(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Operator) {
        return problem.into_response();
    }
    let Some(service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };
    let outcomes = match service
        .release_all_dev_overrides_without_restart(&state.info.app_version)
        .await
    {
        Ok(outcomes) => outcomes,
        Err(err) => {
            return http_problem(
                StatusCode::BAD_REQUEST,
                http_messages::problems::service_operation_failed(err),
            )
            .into_response();
        }
    };
    let released = outcomes
        .into_iter()
        .filter(|(_, outcome)| outcome.dev_override_cleared)
        .map(|(module_id, _)| module_id.to_string())
        .collect();
    (
        StatusCode::OK,
        Json(ReleaseDevOverridesResponse { released }),
    )
        .into_response()
}

async fn proxy_module_service(
    State(state): State<HttpState>,
    AxumPath((service_param, tail)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(original_uri): OriginalUri,
    body: Body,
) -> Response {
    match gateway_proxy(
        state,
        service_param,
        tail,
        method,
        headers,
        original_uri,
        body,
        GatewayRequestKind::Http,
    )
    .await
    {
        Ok(resp) => resp,
        Err(problem) => problem.into_response(),
    }
}

async fn proxy_module_service_grpc(
    State(state): State<HttpState>,
    AxumPath((service_param, tail)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(original_uri): OriginalUri,
    body: Body,
) -> Response {
    match gateway_proxy(
        state,
        service_param,
        tail,
        method,
        headers,
        original_uri,
        body,
        GatewayRequestKind::Grpc,
    )
    .await
    {
        Ok(resp) => resp,
        Err(problem) => problem.into_response(),
    }
}

async fn gateway_proxy(
    state: HttpState,
    service_param: String,
    tail: String,
    method: Method,
    headers: HeaderMap,
    original_uri: Uri,
    body: Body,
    kind: GatewayRequestKind,
) -> Result<Response, ServiceActionProblem> {
    let Some(module_service) = state.services.module_service() else {
        return Err(module_service_unavailable());
    };
    let Some(security) = state.services.security_manager() else {
        return Err(http_problem(
            StatusCode::SERVICE_UNAVAILABLE,
            http_messages::problems::gateway_security_unavailable(),
        ));
    };
    let request_path = original_uri.path().to_string();

    let (result, audit) = async {
        let service_id = match decode_service_id(&service_param) {
            Ok(id) => id,
            Err(problem) => return (Err(problem), None),
        };
        let snapshot = match state.registry.get(&service_id) {
            Some(snapshot) => snapshot,
            None => {
                return (
                    Err(http_problem(
                        StatusCode::NOT_FOUND,
                        http_messages::problems::gateway_unknown_service(&service_id),
                    )),
                    None,
                )
            }
        };
        let ingress_owned = snapshot.descriptor.ingress.clone();
        let ingress = ingress_owned.as_ref();
        let access = ingress
            .map(|meta| meta.access)
            .unwrap_or(ServiceIngressAccess::Internal);
        let audit_ctx = if matches!(access, ServiceIngressAccess::Public) {
            Some(GatewayAuditContext::new(
                Arc::clone(&state.services),
                service_id.clone(),
                method.clone(),
                request_path.clone(),
                access,
                kind,
            ))
        } else {
            None
        };

        if kind == GatewayRequestKind::Grpc {
            let supports_grpc = ingress
                .map(|meta| {
                    meta.protocols
                        .iter()
                        .any(|protocol| matches!(protocol, ServiceIngressProtocol::Grpc))
                })
                .unwrap_or(false);
            if !supports_grpc {
                return (
                    Err(http_problem(
                        StatusCode::BAD_REQUEST,
                        http_messages::problems::gateway_protocol_unsupported(
                            &service_id,
                            kind.as_str(),
                        ),
                    )),
                    audit_ctx,
                );
            }
        }

        let security_metadata = snapshot.descriptor.security.as_ref();
        if matches!(access, ServiceIngressAccess::Internal) && security_metadata.is_none() {
            return (
                Err(http_problem(
                    StatusCode::FORBIDDEN,
                    http_messages::problems::gateway_security_missing(&service_id),
                )),
                audit_ctx,
            );
        }

        let mut claims: Option<DelegatedTokenClaims> = None;
        match access {
            ServiceIngressAccess::Internal => {
                let token = match extract_gateway_token(&headers) {
                    Ok(token) => token,
                    Err(problem) => return (Err(problem), audit_ctx),
                };
                let validated = match security.validate_service_token(token) {
                    Ok(claims) => claims,
                    Err(err) => return (Err(map_service_token_error(err)), audit_ctx),
                };
                if let Some(metadata) = security_metadata {
                    if !metadata.allows_claims(&validated) {
                        return (
                            Err(http_problem(
                                StatusCode::FORBIDDEN,
                                http_messages::problems::gateway_access_denied(&service_id),
                            )),
                            audit_ctx,
                        );
                    }
                }
                claims = Some(validated);
            }
            ServiceIngressAccess::Public => match extract_optional_gateway_token(&headers) {
                Ok(Some(token)) => {
                    let validated = match security.validate_service_token(token) {
                        Ok(claims) => claims,
                        Err(err) => return (Err(map_service_token_error(err)), audit_ctx),
                    };
                    if let Some(metadata) = security_metadata {
                        if !metadata.allows_claims(&validated) {
                            return (
                                Err(http_problem(
                                    StatusCode::FORBIDDEN,
                                    http_messages::problems::gateway_access_denied(&service_id),
                                )),
                                audit_ctx,
                            );
                        }
                    }
                    claims = Some(validated);
                }
                Ok(None) => {}
                Err(problem) => return (Err(problem), audit_ctx),
            },
        }

        let rate_limit_override = ingress.and_then(|meta| match meta.rate_limit {
            ServiceRateLimit::Default => None,
            ServiceRateLimit::Unlimited => Some(0),
            ServiceRateLimit::CustomPerSecond(limit) => Some(limit),
        });
        if !SERVICE_RATE_LIMITER
            .try_acquire_with_limit(&service_id, rate_limit_override)
            .await
        {
            return (
                Err(http_problem(
                    StatusCode::TOO_MANY_REQUESTS,
                    http_messages::problems::gateway_rate_limited(&service_id),
                )),
                audit_ctx,
            );
        }
        let target = match module_service.resolve_ingress_target(&service_id).await {
            Ok(target) => target,
            Err(err) => return (Err(map_ingress_error(err, &service_id)), audit_ctx),
        };
        let path = service_route_path(ingress, &tail, original_uri.query());
        let target_url = build_target_url(&target, &path);
        let reqwest_method = match map_reqwest_method(&method) {
            Ok(method) => method,
            Err(problem) => return (Err(problem), audit_ctx),
        };
        let body_bytes = match collect_request_body(body).await {
            Ok(bytes) => bytes,
            Err(problem) => return (Err(problem), audit_ctx),
        };

        let mut builder = HTTP_GATEWAY_CLIENT
            .request(reqwest_method, target_url)
            .header("x-fenrir-gateway-service", &service_id)
            .header("x-fenrir-gateway-protocol", kind.as_str());

        if let Some(claims) = &claims {
            builder = builder
                .header("x-fenrir-actor", claims.actor.identifier())
                .header("x-fenrir-tenant", claims.tenant_id.as_str());
            if !claims.scopes.is_empty() {
                let scopes = claims
                    .scopes
                    .iter()
                    .map(|scope| scope.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                builder = builder.header("x-fenrir-scopes", scopes);
            }
        } else {
            builder = builder
                .header("x-fenrir-actor", "public")
                .header("x-fenrir-tenant", "public");
        }

        for (name, value) in headers.iter() {
            if name == HOST {
                continue;
            }
            if let (Ok(header_name), Ok(header_value)) = (
                ReqwestHeaderName::from_bytes(name.as_str().as_bytes()),
                ReqwestHeaderValue::from_bytes(value.as_bytes()),
            ) {
                builder = builder.header(header_name, header_value);
            }
        }

        let response = match builder.body(body_bytes).send().await {
            Ok(resp) => resp,
            Err(err) => {
                tracing::warn!(
                    service = %service_id,
                    target = %target_log_label(&target),
                    error = %err,
                    "module gateway upstream request failed"
                );
                return (
                    Err(http_problem(
                        StatusCode::BAD_GATEWAY,
                        http_messages::problems::gateway_upstream_unreachable(&service_id),
                    )),
                    audit_ctx,
                );
            }
        };

        (convert_upstream_response(response).await, audit_ctx)
    }
    .await;

    if let Some(ctx) = audit {
        match &result {
            Ok(resp) => ctx.log(AuditOutcome::Success, resp.status()),
            Err(problem) => ctx.log(AuditOutcome::Failure, problem.status),
        }
    }

    result
}

pub(super) fn build_router(state: HttpState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/static/*path", get(serve_static))
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
        .route("/identity/users", get(list_identity_users))
        .route("/identity/tokens", post(issue_identity_token))
        .route("/modules/installed", get(list_installed_modules))
        .route("/modules/available", get(list_available_modules))
        .route("/modules/install", post(install_module_version))
        .route("/modules/update", post(update_module_versions))
        .route("/modules/:id", delete(uninstall_module_version))
        .route("/modules/runtime/:id/start", post(start_module_runtime))
        .route("/modules/runtime/:id/stop", post(stop_module_runtime))
        .route("/modules/runtime/:id/restart", post(restart_module_runtime))
        .route("/modules/runtime/stop-all", post(stop_all_module_runtimes))
        .route(
            "/modules/runtime/release-dev-overrides",
            post(release_dev_overrides),
        )
        .route("/modules/runtime/tokens", post(issue_module_service_token))
        .route(
            "/gateway/services/:service_id/*path",
            any(proxy_module_service),
        )
        .route(
            "/gateway/grpc/:service_id/*path",
            any(proxy_module_service_grpc),
        )
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
    match tokio::fs::read_to_string("static/index.html").await {
        Ok(content) => Html(content),
        Err(_) => Html(INDEX_HTML.to_string()), // Fallback to embedded HTML
    }
}

async fn serve_static(AxumPath(path): AxumPath<String>) -> impl IntoResponse {
    let file_path = format!("static/{}", path);

    match tokio::fs::read(&file_path).await {
        Ok(content) => {
            let content_type = if file_path.ends_with(".css") {
                "text/css"
            } else if file_path.ends_with(".js") {
                "application/javascript"
            } else if file_path.ends_with(".html") {
                "text/html"
            } else if file_path.ends_with(".png") {
                "image/png"
            } else if file_path.ends_with(".jpg") || file_path.ends_with(".jpeg") {
                "image/jpeg"
            } else if file_path.ends_with(".svg") {
                "image/svg+xml"
            } else {
                "application/octet-stream"
            };

            ([(axum::http::header::CONTENT_TYPE, content_type)], content).into_response()
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
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
    if let Some(response) = ensure_viewer_access(&state, &headers) {
        return response;
    }
    let mut services: Vec<ServiceSummary> = state
        .registry
        .snapshot()
        .into_iter()
        .filter(|snapshot| !is_module_placeholder(&snapshot.descriptor.id))
        .map(snapshot_to_summary)
        .collect();
    services.sort_by(|a, b| a.id.cmp(&b.id));
    (StatusCode::OK, Json(ServicesResponse { services })).into_response()
}

async fn list_scheduler_jobs(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Some(response) = ensure_viewer_access(&state, &headers) {
        return response;
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
    if let Some(response) = ensure_viewer_access(&state, &headers) {
        return response;
    }

    let limit = query.limit.unwrap_or(20).clamp(1, 200);

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
        Err(err) => http_problem(
            StatusCode::INTERNAL_SERVER_ERROR,
            http_messages::problems::audit_unavailable(err),
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
                return viewer_role_required().into_response();
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

async fn list_identity_users(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    match authorize_identity(&state, &headers, Role::Admin) {
        Ok(identity) => match tokio::task::spawn_blocking(move || identity.list_users()).await {
            Ok(Ok(users)) => {
                let response = IdentityUsersResponse {
                    users: users
                        .into_iter()
                        .map(|user| IdentityUserView {
                            user_id: user.user_id.clone(),
                            display_name: user.display_name.clone(),
                            role: user.role.as_str().to_string(),
                            created_at: format_offset_datetime(user.created_at),
                            last_issued_at: user.last_issued_at.map(format_offset_datetime),
                            token_count: user.token_count,
                            tokens: user
                                .tokens
                                .into_iter()
                                .map(|token| IdentityUserTokenView {
                                    token_id: token.token_id,
                                    fingerprint: token.fingerprint,
                                    issued_at: format_offset_datetime(token.issued_at),
                                    expires_at: format_offset_datetime(token.expires_at),
                                    key_id: token.key_id,
                                })
                                .collect(),
                        })
                        .collect(),
                };
                (StatusCode::OK, Json(response)).into_response()
            }
            Ok(Err(err)) => {
                warn!(error = %err, "identity users listing failed");
                http_problem(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    http_messages::problems::identity_list_failed(),
                )
                .into_response()
            }
            Err(err) => {
                warn!(error = %err, "identity users task failed");
                http_problem(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    http_messages::problems::identity_task_failed(),
                )
                .into_response()
            }
        },
        Err(problem) => problem.into_response(),
    }
}

async fn issue_identity_token(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(payload): Json<IssueIdentityTokenPayload>,
) -> Response {
    match authorize_identity(&state, &headers, Role::Admin) {
        Ok(identity) => {
            let role = match payload.role.as_deref() {
                Some(value) => match parse_role_string(value) {
                    Ok(role) => role,
                    Err(problem) => return problem.into_response(),
                },
                None => Role::Admin,
            };

            let request = IssueTokenRequest {
                actor: AuditActor::System,
                user_id: payload.user_id.clone(),
                display_name: payload.display_name.clone(),
                role,
            };
            match tokio::task::spawn_blocking(move || identity.issue_token(request)).await {
                Ok(Ok(issued)) => {
                    let response = IssueIdentityTokenResponse {
                        token: issued.token,
                        token_id: issued.token_id,
                        user_id: payload.user_id,
                        role: role.as_str().to_string(),
                        expires_at: format_offset_datetime(issued.expires_at),
                        fingerprint: issued.fingerprint,
                        issued_at: format_offset_datetime(issued.issued_at),
                    };
                    (StatusCode::OK, Json(response)).into_response()
                }
                Ok(Err(err)) => {
                    warn!(error = %err, "token issuance failed");
                    http_problem(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        http_messages::problems::identity_issue_failed(),
                    )
                    .into_response()
                }
                Err(err) => {
                    warn!(error = %err, "identity token task failed");
                    http_problem(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        http_messages::problems::identity_issue_task_failed(),
                    )
                    .into_response()
                }
            }
        }
        Err(problem) => problem.into_response(),
    }
}

async fn metrics_snapshot() -> impl IntoResponse {
    let snapshot = telemetry::snapshot();
    let (uptime_seconds, counters, service_resources) = if let Some(snapshot) = snapshot {
        let counters = snapshot.metrics.into_iter().collect();
        let resources = snapshot
            .service_resources
            .into_iter()
            .map(service_resource_view)
            .collect();
        (Some(snapshot.uptime.as_secs()), counters, resources)
    } else {
        (None, std::collections::BTreeMap::new(), Vec::new())
    };
    let body = MetricsResponse {
        uptime_seconds,
        live: telemetry::is_live(),
        ready: telemetry::is_ready(),
        counters,
        service_resources,
    };
    Json(body)
}

fn service_resource_view(resource: telemetry::ServiceResourceSnapshot) -> ServiceResourceView {
    let now = OffsetDateTime::now_utc();
    let stale_threshold = TimeDuration::seconds(SERVICE_RESOURCE_STALE_AFTER_SECS);
    let updated_at = format_optional_offset_datetime(resource.updated_at);
    let stale = if resource.reported {
        match resource.updated_at {
            Some(ts) => now - ts >= stale_threshold,
            None => true,
        }
    } else {
        false
    };

    ServiceResourceView {
        id: resource.id,
        cpu_percent: resource.cpu_percent,
        memory_bytes: resource.memory_bytes,
        memory_peak_bytes: resource.memory_peak_bytes,
        updated_at,
        stale,
        reported: resource.reported,
    }
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
                return viewer_role_required().into_response();
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
            format!("Audit store unavailable: {err}"),
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
        return http_problem(
            StatusCode::BAD_REQUEST,
            http_messages::problems::invalid_level_empty(),
        )
        .into_response();
    }

    let role = if state.auth.is_configured() {
        let token = extract_bearer_token(&headers);
        match state.auth.authorize_token(token) {
            Ok(role) => {
                if !role.satisfies(Role::Admin) {
                    return admin_role_required().into_response();
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
            return http_problem(
                StatusCode::NOT_IMPLEMENTED,
                http_messages::problems::logging_reload_unavailable(),
            )
            .into_response();
        }
    };

    if let Err(err) = logging::reload(&handle, level) {
        return http_problem(
            StatusCode::BAD_REQUEST,
            http_messages::problems::logging_reload_failed(err),
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
            let problem: ServiceActionProblem = err.into();
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

impl From<ServiceControlError> for ServiceActionProblem {
    fn from(err: ServiceControlError) -> Self {
        match err {
            ServiceControlError::UnknownService(id) => http_problem(
                StatusCode::NOT_FOUND,
                http_messages::problems::service_unknown(&id),
            ),
            ServiceControlError::NotControllable(id) => http_problem(
                StatusCode::CONFLICT,
                http_messages::problems::service_not_controllable(&id),
            ),
            ServiceControlError::ForceRequired(id) => http_problem(
                StatusCode::PRECONDITION_FAILED,
                http_messages::problems::service_force_required(&id),
            ),
            ServiceControlError::CoreLocked(id) => http_problem(
                StatusCode::CONFLICT,
                http_messages::problems::service_core_locked(&id),
            ),
            ServiceControlError::OperationFailed { source, .. } => http_problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                http_messages::problems::service_operation_failed(source),
            ),
        }
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

#[derive(Clone)]
struct GatewayAuditContext {
    services: Arc<AppServices>,
    service_id: String,
    method: Method,
    path: String,
    access: ServiceIngressAccess,
    protocol: GatewayRequestKind,
}

impl GatewayAuditContext {
    fn new(
        services: Arc<AppServices>,
        service_id: String,
        method: Method,
        path: String,
        access: ServiceIngressAccess,
        protocol: GatewayRequestKind,
    ) -> Self {
        Self {
            services,
            service_id,
            method,
            path,
            access,
            protocol,
        }
    }

    fn log(&self, outcome: AuditOutcome, status: StatusCode) {
        let metadata = AuditMetadata::default()
            .insert("transport", "http")
            .insert("gateway_protocol", self.protocol.as_str())
            .insert("ingress_access", self.access.as_str())
            .insert("method", self.method.as_str())
            .insert("path", &self.path)
            .insert("status", status.as_str());
        push_audit_event(
            &self.services,
            AuditActor::System,
            "gateway.proxy",
            &self.service_id,
            outcome,
            metadata,
        );
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
    let ts = system_time_to_rfc3339(event.timestamp).unwrap_or_else(|| timestamp.to_string());

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
        AuthError::Unauthorized => http_problem(
            StatusCode::UNAUTHORIZED,
            http_messages::problems::unauthorized(),
        ),
        AuthError::Forbidden => {
            http_problem(StatusCode::FORBIDDEN, http_messages::problems::forbidden())
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
            if role.satisfies(required) {
                if let Some(security) = services.security_manager() {
                    security.audit_control_plane_token(token, required, &Ok(role));
                }
                Ok(role)
            } else {
                if let Some(security) = services.security_manager() {
                    security.audit_control_plane_token(token, required, &Err(AuthError::Forbidden));
                }
                Err(http_problem(
                    StatusCode::FORBIDDEN,
                    http_messages::problems::role_insufficient(required.as_str(), role.as_str()),
                ))
            }
        }
        Err(err) => {
            if let Some(security) = services.security_manager() {
                let audit_err = match &err {
                    AuthError::Unauthorized => AuthError::Unauthorized,
                    AuthError::Forbidden => AuthError::Forbidden,
                };
                security.audit_control_plane_token(token, required, &Err(audit_err));
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
        Ok(dt) => match dt.checked_add(TimeDuration::milliseconds(millis)) {
            Some(adjusted) => format_offset_datetime(adjusted),
            None => timestamp_ms.to_string(),
        },
        Err(_) => timestamp_ms.to_string(),
    }
}

fn ensure_viewer_access(state: &HttpState, headers: &HeaderMap) -> Option<Response> {
    if state.auth.is_configured() {
        let token = extract_bearer_token(headers);
        let role = state.auth.authorize_token(token).map_err(map_auth_error);
        match role {
            Ok(role) if role.satisfies(Role::Viewer) => {}
            Ok(_) => return Some(viewer_role_required().into_response()),
            Err(problem) => return Some(problem.into_response()),
        }
    }
    None
}
const INDEX_HTML: &str = include_str!("../../../static/index.html");
