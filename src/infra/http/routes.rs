use std::collections::HashMap;
use std::convert::Infallible;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{ConnectInfo, OriginalUri, Path as AxumPath, Query, State};
use axum::http::{
    header::{
        HeaderName as AxumHeaderName, ACCEPT, ACCESS_CONTROL_ALLOW_ORIGIN, AUTHORIZATION, HOST,
    },
    HeaderMap, HeaderValue, Method, Request, StatusCode, Uri,
};
use axum::middleware::{self, Next};
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
use time::format_description::well_known::Rfc3339;
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
    ModuleRuntimeKind, ModuleSearchQuery, ModuleServiceError, ModuleStorageError, ModuleVersion,
};
use crate::infra::{logging, telemetry};
use crate::security::auth::{AuthError, ControlPlaneAuthorizer, Role};
use crate::security::identity::{IdentityProvider, IssueTokenRequest};
use crate::security::manager::SecurityError;
use crate::security::service::ServiceScope;
use crate::security::service_tokens::{
    DelegatedActor, DelegatedToken, DelegatedTokenClaims, ServiceTokenError,
};
use crate::services::module::token_audit::{record_module_token_exchange, ModuleTokenAuditContext};
use crate::services::scheduler::ScheduledJobSnapshot;
use crate::services::{
    map_service_to_public_component,
    module::{
        ModuleIngressError, ModuleIngressTarget, ModuleServicesPublishRequest, ModuleStartupReport,
        ReportedServiceEntry, ReportedServicesPayload,
    },
    AppServices, PublicComponentKey, PublicComponentStatus, ServiceActionKind, ServiceControlError,
    ServiceIngressAccess, ServiceIngressMetadata, ServiceIngressProtocol, ServiceMetricSnapshot,
    ServiceRateLimit, ServiceRegistry, ServiceRuntimeMetricsSnapshot, ServiceSecurityMetadata,
    ServiceSnapshot, ServiceStatus, TokenExchangeError,
};
use crate::utils::messages::infra::http::{self as http_messages, ProblemText};
use crate::utils::{
    format_offset_datetime, format_optional_offset_datetime, system_time_to_rfc3339,
};
use fenrir_module_kit::{ModuleTokenExchangeRequest, ModuleTokenExchangeResponse};

use super::gateway::{HTTP_GATEWAY_CLIENT, HTTP_GATEWAY_STREAMING_CLIENT, SERVICE_RATE_LIMITER};
use super::server::HTTP_SERVICE_ID;
use super::state::{HttpInfo, HttpState};

const SERVICE_RESOURCE_STALE_AFTER_SECS: i64 = 60;
const SERVICE_HEALTH_STALE_AFTER_SECS: u64 = 180;
const ALLOWED_MODULE_TOKEN_SCOPES: &[&str] = &["db:write"];
const MODULE_TOKEN_ENDPOINT: &str = "/modules/runtime/tokens";
const MODULE_SERVICE_SIGNATURE_MAX_AGE_SECS: i64 = 300;
const MODULE_SERVICE_SIGNATURE_MAX_FUTURE_SECS: i64 = 60;

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
struct PublicStatusResponse {
    overall: &'static str,
    updated_at: String,
    live: bool,
    ready: bool,
    components: Vec<PublicStatusComponent>,
    incidents: Vec<PublicStatusIncident>,
}

#[derive(Serialize)]
struct PublicStatusComponent {
    key: &'static str,
    name: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

#[derive(Serialize)]
struct PublicStatusIncident {
    id: String,
    component: &'static str,
    title: String,
    status: &'static str,
    severity: &'static str,
    started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_at: Option<String>,
}

#[derive(Serialize)]
struct DbRuntimeStatusResponse {
    engine: String,
    running: bool,
    adapter_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_health: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_checkpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_backup_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_backup_state_path: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    applied_migrations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<ServiceHealthView>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<ServiceHealthView>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostics: Option<ServiceHealthView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_metrics: Option<ServiceRuntimeMetricsView>,
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

#[derive(Clone, Serialize)]
struct ServiceHealthView {
    state: &'static str,
    last_heartbeat_seconds: Option<u64>,
    latency_p50_ms: Option<f64>,
    latency_p95_ms: Option<f64>,
    error_rate_pct: Option<f64>,
}

#[derive(Clone, Serialize)]
struct ServiceRuntimeMetricsView {
    updated_at: Option<String>,
    stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<serde_json::Value>,
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

#[derive(Serialize)]
struct ServiceManifestSignaturePayload {
    module_id: String,
    schema_version: String,
    signed_at: String,
    services: Vec<ReportedServiceEntry>,
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

#[derive(Deserialize, Default)]
struct ModuleAnalyticsQuery {
    module: Option<String>,
    level: Option<String>,
    search: Option<String>,
    limit: Option<usize>,
    tail: Option<usize>,
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
struct ModuleAnalyticsResponse {
    filter: ModuleAnalyticsFilterView,
    counts: ModuleAnalyticsCounts,
    modules: Vec<ModuleAnalyticsModuleSummary>,
    events: Vec<ModuleAnalyticsEventView>,
}

#[derive(Serialize)]
struct ModuleAnalyticsFilterView {
    module: Option<String>,
    level: String,
    search: Option<String>,
    limit: usize,
    tail: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
struct ModuleAnalyticsCounts {
    total: usize,
    error: usize,
    warn: usize,
    info: usize,
    debug: usize,
    trace: usize,
    unavailable: usize,
}

#[derive(Serialize)]
struct ModuleAnalyticsModuleSummary {
    module_id: String,
    total: usize,
    error: usize,
    warn: usize,
    available: bool,
    last_event_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ModuleAnalyticsEventView {
    module_id: String,
    level: String,
    timestamp: Option<String>,
    target: Option<String>,
    message: String,
    raw: String,
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
    paused: bool,
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
struct ModuleStartupReportsResponse {
    reports: Vec<ModuleStartupReport>,
}

#[derive(Serialize)]
struct ModuleStartupReportResponse {
    report: ModuleStartupReport,
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

fn snapshot_to_summary(
    svc: ServiceSnapshot,
    metrics: Option<ServiceMetricSnapshot>,
) -> ServiceSummary {
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
        diagnostics: service_health_view(svc.status, metrics),
        runtime_metrics: None,
    }
}

fn overall_public_status(
    live: bool,
    ready: bool,
    components: &[PublicStatusComponent],
) -> &'static str {
    if !live {
        return "major_outage";
    }
    if !ready {
        return "major_outage";
    }
    let any_failed = components
        .iter()
        .any(|component| matches!(component.status, "down"));
    if any_failed {
        return "major_outage";
    }
    let any_degraded = components
        .iter()
        .any(|component| matches!(component.status, "degraded"));
    let all_unknown = components
        .iter()
        .all(|component| matches!(component.status, "unknown"));
    if any_degraded || all_unknown {
        "degraded"
    } else {
        "operational"
    }
}

fn public_status_level(
    status: ServiceStatus,
    metrics: Option<ServiceMetricSnapshot>,
) -> PublicComponentStatus {
    let health = health_state_label(status, metrics);
    match status {
        ServiceStatus::Failed | ServiceStatus::Stopped => PublicComponentStatus::Down,
        ServiceStatus::Degraded | ServiceStatus::Starting => PublicComponentStatus::Degraded,
        ServiceStatus::Standby => PublicComponentStatus::Unknown,
        ServiceStatus::Active => match health {
            "stale" => PublicComponentStatus::Down,
            "degraded" => PublicComponentStatus::Degraded,
            "healthy" | "unknown" => PublicComponentStatus::Up,
            _ => PublicComponentStatus::Unknown,
        },
    }
}

fn component_order(key: PublicComponentKey) -> usize {
    match key {
        PublicComponentKey::Website => 0,
        PublicComponentKey::Account => 1,
        PublicComponentKey::Api => 2,
        PublicComponentKey::Notifications => 3,
    }
}

fn build_public_components(
    services: &[ServiceSnapshot],
    diagnostics: &HashMap<String, ServiceMetricSnapshot>,
) -> Vec<PublicStatusComponent> {
    let mut levels: HashMap<PublicComponentKey, PublicComponentStatus> = HashMap::new();
    let mut seen_notifications = false;

    for snapshot in services {
        let Some(component) = map_service_to_public_component(&snapshot.descriptor.id) else {
            continue;
        };
        if !is_public_status_visible(snapshot) {
            continue;
        }
        if component == PublicComponentKey::Notifications {
            seen_notifications = true;
        }
        let metrics = diagnostics.get(&snapshot.descriptor.id).copied();
        let level = public_status_level(snapshot.status, metrics);
        levels
            .entry(component)
            .and_modify(|value| *value = (*value).max(level))
            .or_insert(level);
    }

    for required in PublicComponentKey::REQUIRED {
        levels
            .entry(required)
            .or_insert(PublicComponentStatus::Unknown);
    }
    if !seen_notifications {
        levels.remove(&PublicComponentKey::Notifications);
    } else {
        levels
            .entry(PublicComponentKey::Notifications)
            .or_insert(PublicComponentStatus::Unknown);
    }

    let mut components: Vec<PublicStatusComponent> = levels
        .into_iter()
        .map(|(key, status)| PublicStatusComponent {
            key: key.as_key(),
            name: key.display_name().to_string(),
            status: status.as_str(),
            updated_at: None,
        })
        .collect();
    components.sort_by_key(|component| {
        let key = match component.key {
            "website" => PublicComponentKey::Website,
            "account" => PublicComponentKey::Account,
            "api" => PublicComponentKey::Api,
            "notifications" => PublicComponentKey::Notifications,
            _ => PublicComponentKey::Notifications,
        };
        component_order(key)
    });
    components
}

fn is_module_placeholder(id: &str) -> bool {
    id.starts_with("module:") && !id.contains("::")
}

fn is_public_status_visible(snapshot: &ServiceSnapshot) -> bool {
    if snapshot.descriptor.id == HTTP_SERVICE_ID {
        return true;
    }
    snapshot
        .descriptor
        .ingress
        .as_ref()
        .map(|ingress| ingress.access == ServiceIngressAccess::Public)
        .unwrap_or(false)
}

fn build_public_incidents(state: &HttpState, limit: usize) -> Vec<PublicStatusIncident> {
    state
        .services
        .public_status_tracker()
        .recent_incidents(limit)
        .into_iter()
        .map(|incident| PublicStatusIncident {
            id: incident.id,
            component: incident.component.as_key(),
            title: incident.title,
            status: incident.status.as_str(),
            severity: incident.severity.as_str(),
            started_at: system_time_to_rfc3339(incident.started_at)
                .unwrap_or_else(|| format_offset_datetime(OffsetDateTime::now_utc())),
            resolved_at: incident.resolved_at.and_then(system_time_to_rfc3339),
        })
        .collect()
}

fn service_health_view(
    status: ServiceStatus,
    metrics: Option<ServiceMetricSnapshot>,
) -> Option<ServiceHealthView> {
    metrics.map(|snapshot| ServiceHealthView {
        state: health_state_label(status, Some(snapshot)),
        last_heartbeat_seconds: snapshot
            .last_heartbeat_elapsed()
            .map(|duration| duration.as_secs()),
        latency_p50_ms: snapshot.latency_p50_ms,
        latency_p95_ms: snapshot.latency_p95_ms,
        error_rate_pct: snapshot.error_rate_pct,
    })
}

fn service_runtime_metrics_view(
    metrics: ServiceRuntimeMetricsSnapshot,
) -> ServiceRuntimeMetricsView {
    ServiceRuntimeMetricsView {
        updated_at: metrics.updated_at.and_then(system_time_to_rfc3339),
        stale: metrics.is_stale(Duration::from_secs(
            crate::services::diagnostics::DEFAULT_RESOURCE_STALE_AFTER_SECS,
        )),
        last_error: metrics.last_error,
        payload: metrics.payload,
    }
}

fn health_state_label(
    status: ServiceStatus,
    metrics: Option<ServiceMetricSnapshot>,
) -> &'static str {
    if let Some(snapshot) = metrics {
        if let Some(last) = snapshot.last_heartbeat_elapsed() {
            if last.as_secs() >= SERVICE_HEALTH_STALE_AFTER_SECS {
                return "stale";
            }
        }
        if let Some(err) = snapshot.error_rate_pct {
            if err >= 5.0 {
                return "degraded";
            }
        }
        return "healthy";
    }
    match status {
        ServiceStatus::Failed | ServiceStatus::Degraded => "degraded",
        _ => "unknown",
    }
}

fn snapshot_to_state_event(
    snapshot: ServiceSnapshot,
    metrics: Option<ServiceMetricSnapshot>,
) -> ServiceStateEvent {
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
        diagnostics: service_health_view(snapshot.status, metrics),
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
        ModuleRuntimeError::EnvUnavailable { .. } => {
            (StatusCode::CONFLICT, "module_env_unavailable")
        }
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

fn normalize_module_id(raw: &str) -> Result<ModuleId, ServiceActionProblem> {
    let value = raw.trim();
    let cleaned = value.strip_prefix("module:").unwrap_or(value);
    ModuleId::new(cleaned).map_err(|_| {
        http_problem(
            StatusCode::BAD_REQUEST,
            http_messages::problems::module_token_invalid_service(value),
        )
    })
}

fn parse_requested_scopes(scopes: &[String]) -> Result<Vec<ServiceScope>, ServiceActionProblem> {
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

fn extract_optional_gateway_token(
    headers: &HeaderMap,
) -> Result<Option<&str>, ServiceActionProblem> {
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
                    if !tail.is_empty() {
                        buffer.push_str(tail);
                    }
                } else {
                    match tail.strip_prefix(&normalized) {
                        Some("") => {
                            buffer.push('/');
                            buffer.push_str(&normalized);
                        }
                        Some(rest) if rest.starts_with('/') => {
                            buffer.push('/');
                            buffer.push_str(tail);
                        }
                        _ => {
                            buffer.push('/');
                            buffer.push_str(&normalized);
                            if !tail.is_empty() {
                                buffer.push('/');
                                buffer.push_str(tail);
                            }
                        }
                    }
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

#[cfg(test)]
mod service_route_path_tests {
    use super::{
        parse_module_analytics_line, service_route_path, should_fail_over_gateway_error,
        should_fail_over_gateway_response, ModuleAnalyticsLevelFilter,
    };
    use crate::services::ServiceIngressMetadata;
    use axum::http::Method;
    use reqwest::Client;

    #[test]
    fn avoids_duplicate_route_prefix_when_client_preprends_it() {
        let ingress = ServiceIngressMetadata::public().with_route_prefix("/api/v1");
        let path = service_route_path(Some(&ingress), "/api/v1/auth/email-code", Some("foo=bar"));
        assert_eq!(path, "/api/v1/auth/email-code?foo=bar");
    }

    #[test]
    fn prepends_route_prefix_when_missing() {
        let ingress = ServiceIngressMetadata::public().with_route_prefix("/api/v1");
        let path = service_route_path(Some(&ingress), "/auth/email-code", None);
        assert_eq!(path, "/api/v1/auth/email-code");
    }

    #[test]
    fn handles_root_route_prefix() {
        let ingress = ServiceIngressMetadata::public().with_route_prefix("/");
        let path = service_route_path(Some(&ingress), "/status", None);
        assert_eq!(path, "/status");
    }

    #[test]
    fn defaults_without_ingress_metadata() {
        let path = service_route_path(None, "/status", None);
        assert_eq!(path, "/status");
    }

    #[test]
    fn failover_only_on_safe_gateway_statuses_for_get() {
        assert!(should_fail_over_gateway_response(
            &Method::GET,
            reqwest::StatusCode::BAD_GATEWAY
        ));
        assert!(should_fail_over_gateway_response(
            &Method::GET,
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(!should_fail_over_gateway_response(
            &Method::POST,
            reqwest::StatusCode::BAD_GATEWAY
        ));
        assert!(!should_fail_over_gateway_response(
            &Method::GET,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
    }

    #[tokio::test]
    async fn failover_only_on_connect_or_timeout_errors() {
        let err = Client::new()
            .get("http://127.0.0.1:9")
            .send()
            .await
            .expect_err("connection should fail");
        assert!(should_fail_over_gateway_error(&err));
    }

    #[test]
    fn parses_structured_module_log_lines() {
        let event = parse_module_analytics_line(
            "athene-api",
            "2026-03-18T11:21:19.141177Z  WARN tokio-runtime-worker ThreadId(09) athene_api::gateway: upstream request failed target=auth-service",
            0,
        )
        .expect("line parsed");
        assert_eq!(event.view.module_id, "athene-api");
        assert_eq!(event.view.level, "warn");
        assert_eq!(event.view.target.as_deref(), Some("athene_api::gateway"));
        assert_eq!(
            event.view.message,
            "upstream request failed target=auth-service"
        );
        assert!(event.view.timestamp.is_some());
    }

    #[test]
    fn analytics_issue_filter_only_matches_warn_and_error() {
        let issues = ModuleAnalyticsLevelFilter::parse(Some("issues"));
        let warn = parse_module_analytics_line(
            "auth-service",
            "2026-03-18T11:21:19.141177Z  WARN main auth::svc: slow downstream",
            0,
        )
        .unwrap();
        let info = parse_module_analytics_line(
            "auth-service",
            "2026-03-18T11:21:19.141177Z  INFO main auth::svc: started",
            1,
        )
        .unwrap();
        assert!(issues.matches(warn.level));
        assert!(!issues.matches(info.level));
    }
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
        ModuleIngressTarget::RuntimePort {
            module_id,
            instance_id,
            port,
        } => {
            format!("module:{}:{}@{}", module_id, instance_id, port)
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

fn should_fail_over_gateway_response(method: &Method, status: reqwest::StatusCode) -> bool {
    method == Method::GET
        && matches!(
            status,
            reqwest::StatusCode::BAD_GATEWAY
                | reqwest::StatusCode::SERVICE_UNAVAILABLE
                | reqwest::StatusCode::GATEWAY_TIMEOUT
        )
}

fn should_fail_over_gateway_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect()
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

    let is_streaming = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("text/event-stream"))
        .unwrap_or(false);

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

    if is_streaming {
        builder
            .body(Body::from_stream(response.bytes_stream()))
            .map_err(|err| {
                http_problem(
                    StatusCode::BAD_GATEWAY,
                    http_messages::problems::gateway_upstream_conversion_failed(err),
                )
            })
    } else {
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
        builder.body(Body::from(body)).map_err(|err| {
            http_problem(
                StatusCode::BAD_GATEWAY,
                http_messages::problems::gateway_upstream_conversion_failed(err),
            )
        })
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

async fn issue_module_service_token(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(payload): Json<ModuleTokenExchangeRequest>,
) -> Response {
    let Some(_module_service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };
    let Some(token_exchange) = state.services.token_exchange_service() else {
        return http_problem(
            StatusCode::SERVICE_UNAVAILABLE,
            http_messages::problems::token_exchange_unavailable(),
        )
        .into_response();
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
    let claims = match security.validate_service_token_for_refresh(token) {
        Ok(claims) => claims,
        Err(err) => return map_service_token_error(err).into_response(),
    };
    let grace_refresh = claims.expires_at <= time::OffsetDateTime::now_utc();
    let module_id = match module_id_from_claims(&claims) {
        Ok(id) => id,
        Err(problem) => return problem.into_response(),
    };
    let ModuleTokenExchangeRequest {
        scopes: requested_scopes,
        reason: request_reason,
    } = payload;
    let effective_reason: Option<String> = if grace_refresh {
        Some(format!(
            "grace_period_refresh{}",
            request_reason
                .as_ref()
                .map(|r| format!(" (original: {r})"))
                .unwrap_or_default()
        ))
    } else {
        request_reason.clone()
    };

    let scopes = match parse_requested_scopes(&requested_scopes) {
        Ok(scopes) => scopes,
        Err(problem) => return problem.into_response(),
    };
    let issued = if scopes.is_empty() {
        match token_exchange.issue_default_token(&module_id).await {
            Ok(token) => token,
            Err(TokenExchangeError::RateLimited) => {
                record_module_token_exchange(
                    state.services.as_ref(),
                    &module_id,
                    ModuleTokenAuditContext {
                        transport: "control-plane",
                        endpoint: Some(MODULE_TOKEN_ENDPOINT),
                        requested_scopes: &requested_scopes,
                        granted_scopes: None,
                        reason: effective_reason.as_deref(),
                        expires_in_seconds: 0,
                        outcome: AuditOutcome::Failure,
                        error: Some("rate_limited"),
                    },
                );
                return http_problem(
                    StatusCode::TOO_MANY_REQUESTS,
                    http_messages::problems::token_exchange_rate_limited(),
                )
                .into_response();
            }
            Err(TokenExchangeError::Module(err)) => {
                let err_text = err.to_string();
                record_module_token_exchange(
                    state.services.as_ref(),
                    &module_id,
                    ModuleTokenAuditContext {
                        transport: "control-plane",
                        endpoint: Some(MODULE_TOKEN_ENDPOINT),
                        requested_scopes: &requested_scopes,
                        granted_scopes: None,
                        reason: effective_reason.as_deref(),
                        expires_in_seconds: 0,
                        outcome: AuditOutcome::Failure,
                        error: Some(err_text.as_str()),
                    },
                );
                return http_problem(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    http_messages::problems::module_token_issue_failed(&err),
                )
                .into_response();
            }
        }
    } else {
        match token_exchange.issue_scoped_token(&module_id, scopes).await {
            Ok(token) => token,
            Err(TokenExchangeError::RateLimited) => {
                record_module_token_exchange(
                    state.services.as_ref(),
                    &module_id,
                    ModuleTokenAuditContext {
                        transport: "control-plane",
                        endpoint: Some(MODULE_TOKEN_ENDPOINT),
                        requested_scopes: &requested_scopes,
                        granted_scopes: None,
                        reason: effective_reason.as_deref(),
                        expires_in_seconds: 0,
                        outcome: AuditOutcome::Failure,
                        error: Some("rate_limited"),
                    },
                );
                return http_problem(
                    StatusCode::TOO_MANY_REQUESTS,
                    http_messages::problems::token_exchange_rate_limited(),
                )
                .into_response();
            }
            Err(TokenExchangeError::Module(err)) => {
                let err_text = err.to_string();
                record_module_token_exchange(
                    state.services.as_ref(),
                    &module_id,
                    ModuleTokenAuditContext {
                        transport: "control-plane",
                        endpoint: Some(MODULE_TOKEN_ENDPOINT),
                        requested_scopes: &requested_scopes,
                        granted_scopes: None,
                        reason: effective_reason.as_deref(),
                        expires_in_seconds: 0,
                        outcome: AuditOutcome::Failure,
                        error: Some(err_text.as_str()),
                    },
                );
                return http_problem(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    http_messages::problems::module_token_issue_failed(&err),
                )
                .into_response();
            }
        }
    };
    let DelegatedToken { token, claims } = issued;
    let expires_in_seconds = token_expires_in_seconds(&claims);
    let response = ModuleTokenExchangeResponse {
        token,
        scopes: claims
            .scopes
            .iter()
            .map(|scope| scope.as_str().to_string())
            .collect(),
        expires_in_seconds,
    };
    record_module_token_exchange(
        state.services.as_ref(),
        &module_id,
        ModuleTokenAuditContext {
            transport: "control-plane",
            endpoint: Some(MODULE_TOKEN_ENDPOINT),
            requested_scopes: &requested_scopes,
            granted_scopes: Some(&claims.scopes),
            reason: effective_reason.as_deref(),
            expires_in_seconds,
            outcome: AuditOutcome::Success,
            error: None,
        },
    );
    (StatusCode::OK, Json(response)).into_response()
}

async fn register_module_services(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(payload): Json<ModuleServicesPublishRequest>,
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
    let payload_module_id = match normalize_module_id(&payload.module_id) {
        Ok(id) => id,
        Err(problem) => return problem.into_response(),
    };
    if module_id != payload_module_id {
        return http_problem(
            StatusCode::FORBIDDEN,
            http_messages::problems::module_token_invalid_service(&payload.module_id),
        )
        .into_response();
    }

    let signed_at_raw = match payload.signed_at.as_deref() {
        Some(value) => match OffsetDateTime::parse(value, &Rfc3339) {
            Ok(parsed) => parsed,
            Err(_) => {
                return http_problem(
                    StatusCode::BAD_REQUEST,
                    http_messages::problems::module_token_invalid_service("invalid signed_at"),
                )
                .into_response();
            }
        },
        None => {
            return http_problem(
                StatusCode::BAD_REQUEST,
                http_messages::problems::module_token_invalid_service("signed_at missing"),
            )
            .into_response();
        }
    };
    let now = OffsetDateTime::now_utc();
    if (now - signed_at_raw).whole_seconds() > MODULE_SERVICE_SIGNATURE_MAX_AGE_SECS
        || (signed_at_raw - now).whole_seconds() > MODULE_SERVICE_SIGNATURE_MAX_FUTURE_SECS
    {
        return http_problem(
            StatusCode::UNAUTHORIZED,
            http_messages::problems::module_token_invalid_service("signature expired"),
        )
        .into_response();
    }
    let signature = match payload.signature.as_deref() {
        Some(value) if !value.trim().is_empty() => value,
        _ => {
            return http_problem(
                StatusCode::BAD_REQUEST,
                http_messages::problems::module_token_invalid_service("signature missing"),
            )
            .into_response();
        }
    };
    let schema_version = payload
        .schema_version
        .clone()
        .unwrap_or_else(|| "1.0".to_string());
    let signature_payload = ServiceManifestSignaturePayload {
        module_id: payload.module_id.clone(),
        schema_version,
        signed_at: payload.signed_at.clone().unwrap_or_default(),
        services: payload.services.clone(),
    };
    let signature_bytes = match serde_json::to_vec(&signature_payload) {
        Ok(data) => data,
        Err(err) => {
            return http_problem(
                StatusCode::BAD_REQUEST,
                http_messages::problems::module_token_invalid_service(&err.to_string()),
            )
            .into_response();
        }
    };
    match security.verify_service_manifest_signature(token, &signature_bytes, signature) {
        Ok(true) => {}
        Ok(false) => {
            return http_problem(
                StatusCode::FORBIDDEN,
                http_messages::problems::module_token_invalid_service("signature invalid"),
            )
            .into_response();
        }
        Err(err) => {
            return http_problem(
                StatusCode::INTERNAL_SERVER_ERROR,
                http_messages::problems::module_token_invalid_service(&err.to_string()),
            )
            .into_response();
        }
    }

    let payload = ReportedServicesPayload {
        services: payload.services,
    };
    match module_service
        .register_reported_services(&module_id, payload)
        .await
    {
        Ok(()) => (StatusCode::OK, Json("ok")).into_response(),
        Err(err) => module_runtime_problem(err).into_response(),
    }
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

async fn module_runtime_instances(
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
    match service.runtime_instances(&module_id).await {
        Ok(instances) => (StatusCode::OK, Json(instances)).into_response(),
        Err(err) => module_runtime_problem(err).into_response(),
    }
}

async fn rolling_restart_module_runtime(
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
    match service.rolling_restart(&module_id).await {
        Ok(report) => (StatusCode::OK, Json(report)).into_response(),
        Err(err) => module_runtime_problem(err).into_response(),
    }
}

async fn list_module_startup_reports(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Operator) {
        return problem.into_response();
    }
    let Some(service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };
    let reports = service.startup_reports().await;
    (
        StatusCode::OK,
        Json(ModuleStartupReportsResponse { reports }),
    )
        .into_response()
}

async fn module_startup_report(
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
    let Some(report) = service.startup_report(&module_id).await else {
        return ServiceActionProblem::new(
            StatusCode::NOT_FOUND,
            "module_startup_report_not_found",
            format!("no startup report found for module '{module_id}'"),
        )
        .into_response();
    };
    (StatusCode::OK, Json(ModuleStartupReportResponse { report })).into_response()
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

struct GatewayRoute {
    service_param: String,
    tail: String,
}

struct OptionalPeerAddr(Option<std::net::SocketAddr>);

#[axum::async_trait]
impl<S: Send + Sync> axum::extract::FromRequestParts<S> for OptionalPeerAddr {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<std::net::SocketAddr>>()
                .map(|ci| ci.0),
        ))
    }
}

fn inject_forwarded_for(headers: &mut HeaderMap, peer: std::net::SocketAddr) {
    if headers.contains_key("x-forwarded-for") {
        return;
    }
    if let Ok(value) = HeaderValue::from_str(&peer.ip().to_string()) {
        headers.insert("x-forwarded-for", value);
    }
}

async fn proxy_module_service(
    State(state): State<HttpState>,
    OptionalPeerAddr(peer_addr): OptionalPeerAddr,
    AxumPath((service_param, tail)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(original_uri): OriginalUri,
    body: Body,
) -> Response {
    let mut headers = headers;
    if let Some(addr) = peer_addr {
        inject_forwarded_for(&mut headers, addr);
    }
    let route = GatewayRoute {
        service_param,
        tail,
    };
    match gateway_proxy(
        state,
        route,
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
    OptionalPeerAddr(peer_addr): OptionalPeerAddr,
    AxumPath((service_param, tail)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(original_uri): OriginalUri,
    body: Body,
) -> Response {
    let mut headers = headers;
    if let Some(addr) = peer_addr {
        inject_forwarded_for(&mut headers, addr);
    }
    let route = GatewayRoute {
        service_param,
        tail,
    };
    match gateway_proxy(
        state,
        route,
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

async fn proxy_static_module_root(
    State(state): State<HttpState>,
    AxumPath(module_id_param): AxumPath<String>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(original_uri): OriginalUri,
    body: Body,
) -> Response {
    proxy_static_module_inner(
        state,
        module_id_param,
        String::new(),
        method,
        headers,
        original_uri,
        body,
    )
    .await
}

async fn proxy_static_module(
    State(state): State<HttpState>,
    AxumPath((module_id_param, tail)): AxumPath<(String, String)>,
    method: Method,
    headers: HeaderMap,
    OriginalUri(original_uri): OriginalUri,
    body: Body,
) -> Response {
    proxy_static_module_inner(
        state,
        module_id_param,
        tail,
        method,
        headers,
        original_uri,
        body,
    )
    .await
}

async fn proxy_static_module_inner(
    state: HttpState,
    module_id_param: String,
    tail: String,
    method: Method,
    headers: HeaderMap,
    original_uri: Uri,
    body: Body,
) -> Response {
    match proxy_static_module_impl(
        state,
        module_id_param,
        tail,
        method,
        headers,
        original_uri,
        body,
    )
    .await
    {
        Ok(resp) => resp,
        Err(problem) => problem.into_response(),
    }
}

async fn proxy_static_module_impl(
    state: HttpState,
    module_id_param: String,
    tail: String,
    method: Method,
    headers: HeaderMap,
    original_uri: Uri,
    body: Body,
) -> Result<Response, ServiceActionProblem> {
    let module_id = ModuleId::new(module_id_param.clone()).map_err(|_| {
        http_problem(
            StatusCode::BAD_REQUEST,
            http_messages::problems::static_module_invalid(&module_id_param),
        )
    })?;

    let Some(module_service) = state.services.module_service() else {
        return Err(module_service_unavailable());
    };

    let runtime_info =
        module_service
            .runtime_status(&module_id)
            .await
            .map_err(|err| match err {
                ModuleRuntimeError::NotRunning { .. } => http_problem(
                    StatusCode::NOT_FOUND,
                    http_messages::problems::static_module_not_running(module_id.as_str()),
                ),
                other => http_problem(
                    StatusCode::BAD_GATEWAY,
                    http_messages::problems::service_operation_failed(other),
                ),
            })?;

    if runtime_info.kind != ModuleRuntimeKind::StaticSite {
        return Err(http_problem(
            StatusCode::BAD_REQUEST,
            http_messages::problems::static_module_not_static(module_id.as_str()),
        ));
    }

    let port = runtime_info.port.ok_or_else(|| {
        http_problem(
            StatusCode::BAD_GATEWAY,
            http_messages::problems::static_module_not_running(module_id.as_str()),
        )
    })?;

    let reqwest_method = map_reqwest_method(&method)?;
    let body_bytes = collect_request_body(body).await?;
    let mut target = format!("http://127.0.0.1:{}{}", port, normalize_static_path(&tail));
    if let Some(query) = original_uri.query() {
        target.push('?');
        target.push_str(query);
    }

    let mut builder = HTTP_GATEWAY_CLIENT.request(reqwest_method, target);
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

    let response = builder.body(body_bytes).send().await.map_err(|err| {
        tracing::warn!(
            module = %module_id,
            port,
            error = %err,
            "static module upstream request failed"
        );
        http_problem(
            StatusCode::BAD_GATEWAY,
            http_messages::problems::static_module_unreachable(module_id.as_str()),
        )
    })?;
    convert_upstream_response(response).await
}

fn normalize_static_path(tail: &str) -> String {
    let trimmed = tail.trim_start_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", trimmed)
    }
}

async fn gateway_proxy(
    state: HttpState,
    route: GatewayRoute,
    method: Method,
    headers: HeaderMap,
    original_uri: Uri,
    body: Body,
    kind: GatewayRequestKind,
) -> Result<Response, ServiceActionProblem> {
    let GatewayRoute {
        service_param,
        tail,
    } = route;
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
        if snapshot.status != ServiceStatus::Active {
            return (
                Err(http_problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    http_messages::problems::gateway_service_unavailable(&service_id),
                )),
                None,
            );
        }
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
                        Ok(claims) => Some(claims),
                        Err(SecurityError::ServiceToken(
                            ServiceTokenError::NotFound
                            | ServiceTokenError::Expired
                            | ServiceTokenError::IdleTimeout,
                        )) => None,
                        Err(err) => return (Err(map_service_token_error(err)), audit_ctx),
                    };
                    if let Some(validated) = validated {
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
        let targets = match module_service.resolve_ingress_targets(&service_id).await {
            Ok(targets) => targets,
            Err(err) => return (Err(map_ingress_error(err, &service_id)), audit_ctx),
        };
        let path = service_route_path(ingress, &tail, original_uri.query());
        let reqwest_method = match map_reqwest_method(&method) {
            Ok(method) => method,
            Err(problem) => return (Err(problem), audit_ctx),
        };
        let body_bytes = match collect_request_body(body).await {
            Ok(bytes) => bytes,
            Err(problem) => return (Err(problem), audit_ctx),
        };

        let accepts_sse = headers
            .get(ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("text/event-stream"))
            .unwrap_or(false);

        let client = if accepts_sse {
            &*HTTP_GATEWAY_STREAMING_CLIENT
        } else {
            &*HTTP_GATEWAY_CLIENT
        };
        let request_method = method.clone();
        let reqwest_method_template = reqwest_method.clone();
        let mut saw_transport_failure = false;

        for (index, target) in targets.iter().enumerate() {
            let target_url = build_target_url(target, &path);
            let target_label = target_log_label(target);
            let mut builder = client
                .request(reqwest_method_template.clone(), target_url)
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

            let response = match builder.body(body_bytes.clone()).send().await {
                Ok(resp) => resp,
                Err(err) => {
                    tracing::warn!(
                        service = %service_id,
                        target = %target_label,
                        error = %err,
                        "module gateway upstream request failed"
                    );
                    if should_fail_over_gateway_error(&err) && index + 1 < targets.len() {
                        saw_transport_failure = true;
                        continue;
                    }
                    return (
                        Err(http_problem(
                            StatusCode::BAD_GATEWAY,
                            http_messages::problems::gateway_upstream_unreachable(&service_id),
                        )),
                        audit_ctx,
                    );
                }
            };

            if should_fail_over_gateway_response(&request_method, response.status())
                && index + 1 < targets.len()
            {
                tracing::warn!(
                    service = %service_id,
                    target = %target_label,
                    status = %response.status(),
                    "module gateway upstream returned failover-eligible status"
                );
                continue;
            }

            return (convert_upstream_response(response).await, audit_ctx);
        }

        let _ = saw_transport_failure;
        (
            Err(http_problem(
                StatusCode::BAD_GATEWAY,
                http_messages::problems::gateway_upstream_unreachable(&service_id),
            )),
            audit_ctx,
        )
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
    let middleware_state = state.clone();
    Router::new()
        .route("/", get(index))
        .route("/static/*path", get(serve_static))
        .route("/info", get(info))
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/public/status", get(public_status))
        .route("/services", get(list_services))
        .route(
            "/services/:id/runtime-metrics",
            get(service_runtime_metrics),
        )
        .route("/services/:id/start", post(start_service))
        .route("/services/:id/stop", post(stop_service))
        .route("/services/:id/restart", post(restart_service))
        .route("/services/actions/start-all", post(start_all_services))
        .route("/services/actions/stop-all", post(stop_all_services))
        .route("/services/actions/restart-all", post(restart_all_services))
        .route("/services/db-runtime/status", get(db_runtime_status))
        .route("/services/db-runtime/logs", get(db_runtime_logs))
        .route("/scheduler/jobs", get(list_scheduler_jobs))
        .route("/logging/level", post(update_logging_level))
        .route("/metrics", get(metrics_snapshot))
        .route("/metrics/history", get(metrics_history))
        .route("/analytics/module-events", get(module_analytics))
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
        .route(
            "/modules/runtime/:id/instances",
            get(module_runtime_instances),
        )
        .route(
            "/modules/runtime/:id/rolling-restart",
            post(rolling_restart_module_runtime),
        )
        .route(
            "/modules/runtime/startup-reports",
            get(list_module_startup_reports),
        )
        .route(
            "/modules/runtime/:id/startup-report",
            get(module_startup_report),
        )
        .route("/modules/runtime/stop-all", post(stop_all_module_runtimes))
        .route(
            "/modules/runtime/release-dev-overrides",
            post(release_dev_overrides),
        )
        .route("/modules/runtime/services", post(register_module_services))
        .route("/modules/static/:module_id", any(proxy_static_module_root))
        .route("/modules/static/:module_id/*path", any(proxy_static_module))
        .route("/modules/runtime/tokens", post(issue_module_service_token))
        .route(
            "/gateway/services/:service_id/*path",
            any(proxy_module_service),
        )
        .route(
            "/gateway/grpc/:service_id/*path",
            any(proxy_module_service_grpc),
        )
        .layer(middleware::from_fn_with_state(
            middleware_state,
            http_metrics_middleware,
        ))
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

async fn http_metrics_middleware(
    State(state): State<HttpState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let started_at = Instant::now();
    let response = next.run(req).await;
    let success = !response.status().is_server_error();
    let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
    state
        .services
        .diagnostics()
        .record_probe(HTTP_SERVICE_ID, latency_ms, success);
    response
}

pub fn admin_console_html() -> &'static str {
    INDEX_HTML
}

async fn index() -> impl IntoResponse {
    if let Ok(content) = tokio::fs::read_to_string("static/control-plane/index.html").await {
        return Html(content);
    }
    match tokio::fs::read_to_string("static/index.html").await {
        Ok(content) => Html(content),
        Err(_) => Html(INDEX_HTML.to_string()),
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

async fn public_status(State(state): State<HttpState>) -> impl IntoResponse {
    let diagnostics = state.services.service_diagnostics_snapshot();
    let services = state.registry.snapshot();
    let components = build_public_components(&services, &diagnostics);
    let incidents = build_public_incidents(&state, 12);

    let live = telemetry::is_live();
    let ready = telemetry::is_ready();
    let overall = overall_public_status(live, ready, &components);
    let updated_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| format_offset_datetime(OffsetDateTime::now_utc()));

    let mut response = (
        StatusCode::OK,
        Json(PublicStatusResponse {
            overall,
            updated_at,
            live,
            ready,
            components,
            incidents,
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
}

async fn list_services(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Some(response) = ensure_viewer_access(&state, &headers) {
        return response;
    }
    let diagnostics = state.services.service_diagnostics_snapshot();
    let mut services: Vec<ServiceSummary> = state
        .registry
        .snapshot()
        .into_iter()
        .filter(|snapshot| !is_module_placeholder(&snapshot.descriptor.id))
        .map(|snapshot| {
            let metrics = diagnostics.get(&snapshot.descriptor.id).copied();
            snapshot_to_summary(snapshot, metrics)
        })
        .collect();
    services.sort_by(|a, b| a.id.cmp(&b.id));
    (StatusCode::OK, Json(ServicesResponse { services })).into_response()
}

async fn service_runtime_metrics(
    State(state): State<HttpState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if let Some(response) = ensure_viewer_access(&state, &headers) {
        return response;
    }
    if state.registry.get(&id).is_none() {
        return http_problem(
            StatusCode::NOT_FOUND,
            http_messages::problems::service_unknown(&id),
        )
        .into_response();
    }
    match state.services.service_runtime_metrics(&id) {
        Some(metrics) => {
            (StatusCode::OK, Json(service_runtime_metrics_view(metrics))).into_response()
        }
        None => http_problem(
            StatusCode::NOT_FOUND,
            ProblemText::new("runtime_metrics_unavailable", "runtime metrics unavailable"),
        )
        .into_response(),
    }
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
            paused: job.paused,
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
    let services_handle = Arc::clone(&state.services);
    let service_stream =
        BroadcastStream::new(service_receiver).filter_map(move |result| match result {
            Ok(snapshot) => {
                let metrics = services_handle.service_diagnostics(&snapshot.descriptor.id);
                match serde_json::to_string(&snapshot_to_state_event(snapshot, metrics)) {
                    Ok(json) => Some(Ok::<Event, Infallible>(
                        Event::default().event("service-state").data(json),
                    )),
                    Err(err) => {
                        warn!(error = %err, "failed to encode service event for sse");
                        None
                    }
                }
            }
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

async fn module_analytics(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<ModuleAnalyticsQuery>,
) -> Response {
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Viewer) {
        return problem.into_response();
    }

    let Some(service) = state.services.module_service() else {
        return module_service_unavailable().into_response();
    };

    let level_filter = ModuleAnalyticsLevelFilter::parse(query.level.as_deref());
    let selected_module = query
        .module
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "all")
        .map(str::to_string);
    let search = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let search_normalized = search.as_ref().map(|value| value.to_lowercase());
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let tail = query.tail.unwrap_or(250).clamp(50, 2000);

    let modules = match service.list_installed().await {
        Ok(modules) => modules,
        Err(err) => return module_error_problem(err).into_response(),
    };

    let mut summaries = Vec::new();
    let mut parsed_events = Vec::new();
    let mut counts = ModuleAnalyticsCounts::default();
    let mut sequence = 0usize;

    for installed in modules {
        let Ok(module_id) = installed.manifest.module_id() else {
            continue;
        };
        if selected_module
            .as_deref()
            .map(|selected| selected != module_id.as_str())
            .unwrap_or(false)
        {
            continue;
        }

        match service.logs(&module_id, Some(tail)).await {
            Ok(lines) => {
                let mut module_events = Vec::new();
                for line in lines {
                    let Some(parsed) =
                        parse_module_analytics_line(module_id.as_str(), &line, sequence)
                    else {
                        sequence = sequence.saturating_add(1);
                        continue;
                    };
                    sequence = sequence.saturating_add(1);
                    if !level_filter.matches(parsed.level) {
                        continue;
                    }
                    if !matches_module_analytics_search(&parsed.view, search_normalized.as_deref())
                    {
                        continue;
                    }
                    module_events.push(parsed);
                }

                let mut module_counts = ModuleAnalyticsCounts::default();
                let mut last_event_at = None;
                for event in &module_events {
                    module_counts.record(event.level);
                    last_event_at =
                        latest_optional_timestamp(last_event_at, event.sort_timestamp_ms);
                }

                counts.total += module_counts.total;
                counts.error += module_counts.error;
                counts.warn += module_counts.warn;
                counts.info += module_counts.info;
                counts.debug += module_counts.debug;
                counts.trace += module_counts.trace;

                summaries.push(ModuleAnalyticsModuleSummary {
                    module_id: module_id.to_string(),
                    total: module_counts.total,
                    error: module_counts.error,
                    warn: module_counts.warn,
                    available: true,
                    last_event_at: last_event_at.and_then(format_timestamp_millis),
                    note: None,
                });

                parsed_events.extend(module_events);
            }
            Err(err) => {
                counts.unavailable += 1;
                summaries.push(ModuleAnalyticsModuleSummary {
                    module_id: module_id.to_string(),
                    total: 0,
                    error: 0,
                    warn: 0,
                    available: false,
                    last_event_at: None,
                    note: Some(err.to_string()),
                });
            }
        }
    }

    parsed_events.sort_by(|left, right| {
        right
            .sort_timestamp_ms
            .cmp(&left.sort_timestamp_ms)
            .then_with(|| right.sequence.cmp(&left.sequence))
    });
    let events = parsed_events
        .into_iter()
        .take(limit)
        .map(|event| event.view)
        .collect();
    summaries.sort_by(|left, right| {
        right
            .error
            .cmp(&left.error)
            .then_with(|| right.warn.cmp(&left.warn))
            .then_with(|| right.total.cmp(&left.total))
            .then_with(|| left.module_id.cmp(&right.module_id))
    });

    Json(ModuleAnalyticsResponse {
        filter: ModuleAnalyticsFilterView {
            module: selected_module,
            level: level_filter.as_str().to_string(),
            search,
            limit,
            tail,
        },
        counts,
        modules: summaries,
        events,
    })
    .into_response()
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

async fn db_runtime_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // viewer role reicht für Status
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Viewer) {
        return problem.into_response();
    }
    let tail = params
        .get("tail")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let Some(status) = state.services.db_runtime_status() else {
        return (StatusCode::NOT_FOUND, "not found".to_string()).into_response();
    };
    let diag = state.services.diagnostics().snapshot("db-runtime");
    let resp = DbRuntimeStatusResponse {
        engine: status.engine.as_str().to_string(),
        running: status.running,
        adapter_status: status.adapter_status.as_str().to_string(),
        uri: status.connector_uri,
        port: status.port,
        pid: status.pid,
        last_health: status.last_health.map(format_offset_datetime),
        last_checkpoint: status.last_checkpoint.map(format_offset_datetime),
        snapshot_updated_at: status.snapshot_updated_at.map(format_offset_datetime),
        last_backup_path: status.last_backup_path,
        last_backup_state_path: status.last_backup_state_path,
        applied_migrations: status.applied_migrations,
        logs: if tail > 0 {
            Some(state.services.db_runtime_logs(tail))
        } else {
            None
        },
        diagnostics: diag.map(|d| ServiceHealthView {
            state: if d.error_rate_pct.unwrap_or(0.0) > 0.0 {
                "degraded"
            } else {
                "healthy"
            },
            last_heartbeat_seconds: d.last_heartbeat_elapsed().map(|v| v.as_secs()),
            latency_p50_ms: d.latency_p50_ms,
            latency_p95_ms: d.latency_p95_ms,
            error_rate_pct: d.error_rate_pct,
        }),
    };
    Json(resp).into_response()
}

async fn db_runtime_logs(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // viewer role reicht für Logs
    if let Err(problem) = authorize(&state.auth, &state.services, &headers, Role::Viewer) {
        return problem.into_response();
    }
    let tail = params
        .get("tail")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50);
    let logs = state.services.db_runtime_logs(tail.max(1));
    Json(logs).into_response()
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
            let metrics = services.service_diagnostics(&id);
            let snapshot = registry
                .get(&id)
                .map(|svc| snapshot_to_summary(svc, metrics));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleAnalyticsLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl ModuleAnalyticsLevel {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "TRACE" => Some(Self::Trace),
            "DEBUG" => Some(Self::Debug),
            "INFO" => Some(Self::Info),
            "WARN" | "WARNING" => Some(Self::Warn),
            "ERROR" => Some(Self::Error),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleAnalyticsLevelFilter {
    Issues,
    All,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl ModuleAnalyticsLevelFilter {
    fn parse(value: Option<&str>) -> Self {
        match value
            .unwrap_or("issues")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "all" => Self::All,
            "error" => Self::Error,
            "warn" | "warning" => Self::Warn,
            "info" => Self::Info,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => Self::Issues,
        }
    }

    fn matches(self, level: ModuleAnalyticsLevel) -> bool {
        match self {
            Self::Issues => matches!(
                level,
                ModuleAnalyticsLevel::Warn | ModuleAnalyticsLevel::Error
            ),
            Self::All => true,
            Self::Error => level == ModuleAnalyticsLevel::Error,
            Self::Warn => level == ModuleAnalyticsLevel::Warn,
            Self::Info => level == ModuleAnalyticsLevel::Info,
            Self::Debug => level == ModuleAnalyticsLevel::Debug,
            Self::Trace => level == ModuleAnalyticsLevel::Trace,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Issues => "issues",
            Self::All => "all",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone)]
struct ParsedModuleAnalyticsEvent {
    level: ModuleAnalyticsLevel,
    sort_timestamp_ms: Option<i128>,
    sequence: usize,
    view: ModuleAnalyticsEventView,
}

impl ModuleAnalyticsCounts {
    fn record(&mut self, level: ModuleAnalyticsLevel) {
        self.total += 1;
        match level {
            ModuleAnalyticsLevel::Error => self.error += 1,
            ModuleAnalyticsLevel::Warn => self.warn += 1,
            ModuleAnalyticsLevel::Info => self.info += 1,
            ModuleAnalyticsLevel::Debug => self.debug += 1,
            ModuleAnalyticsLevel::Trace => self.trace += 1,
        }
    }
}

fn parse_module_analytics_line(
    module_id: &str,
    raw_line: &str,
    sequence: usize,
) -> Option<ParsedModuleAnalyticsEvent> {
    let raw = raw_line.trim();
    if raw.is_empty() {
        return None;
    }

    let level = raw
        .split_whitespace()
        .find_map(ModuleAnalyticsLevel::parse)?;
    let timestamp = raw
        .split_whitespace()
        .next()
        .and_then(parse_rfc3339_timestamp);
    let (prefix, message) = raw
        .split_once(": ")
        .map(|(left, right)| (left, right.to_string()))
        .unwrap_or((raw, raw.to_string()));
    let target = prefix
        .split_whitespace()
        .last()
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case(level.as_str()))
        .map(str::to_string);

    Some(ParsedModuleAnalyticsEvent {
        level,
        sort_timestamp_ms: timestamp,
        sequence,
        view: ModuleAnalyticsEventView {
            module_id: module_id.to_string(),
            level: level.as_str().to_string(),
            timestamp: timestamp.and_then(format_timestamp_millis),
            target,
            message,
            raw: raw.to_string(),
        },
    })
}

fn matches_module_analytics_search(event: &ModuleAnalyticsEventView, search: Option<&str>) -> bool {
    let Some(search) = search else {
        return true;
    };
    let search = search.trim();
    if search.is_empty() {
        return true;
    }
    let module = event.module_id.to_lowercase();
    let level = event.level.to_lowercase();
    let target = event.target.as_deref().unwrap_or("").to_lowercase();
    let message = event.message.to_lowercase();
    let raw = event.raw.to_lowercase();
    module.contains(search)
        || level.contains(search)
        || target.contains(search)
        || message.contains(search)
        || raw.contains(search)
}

fn parse_rfc3339_timestamp(value: &str) -> Option<i128> {
    OffsetDateTime::parse(value.trim(), &Rfc3339)
        .ok()
        .map(|timestamp| timestamp.unix_timestamp_nanos())
}

fn format_timestamp_millis(value: i128) -> Option<String> {
    let seconds = (value / 1_000_000_000) as i64;
    let nanos = (value.rem_euclid(1_000_000_000)) as u32;
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|timestamp| timestamp.replace_nanosecond(nanos).ok())
        .and_then(|timestamp| timestamp.format(&Rfc3339).ok())
}

fn latest_optional_timestamp(current: Option<i128>, next: Option<i128>) -> Option<i128> {
    match (current, next) {
        (Some(current), Some(next)) => Some(current.max(next)),
        (Some(current), None) => Some(current),
        (None, Some(next)) => Some(next),
        (None, None) => None,
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
