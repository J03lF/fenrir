use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use once_cell::sync::Lazy;
use serde_json::Value as JsonValue;
use tracing::warn;

use crate::domain::db::{DbEngine, DbError, DbExecutionResult, DbValue};
use crate::security::manager::{SecurityError, SecurityManager};
use crate::security::service::{ServiceScope, ServiceScopeError};
use crate::security::service_tokens::{DelegatedActor, DelegatedTokenClaims};
use crate::services::db_shell::DbShellService;
use crate::services::ServiceDiagnostics;
use crate::utils::messages::services::db_connector::{
    errors as db_connector_errors, logs as db_connector_logs,
};
use fenrir_module_kit::{
    DbConnectorCommand, DbConnectorIntent, DbConnectorRequest, DbConnectorResponse,
    DbConnectorResultView, DbPreparedParam, DbTenantBindingMode, DbTenantPolicy,
};

const MAX_STATEMENT_LEN: usize = 32 * 1024;

static DB_SCOPE_READ: Lazy<ServiceScope> =
    Lazy::new(|| ServiceScope::new("db:read").expect("db:read scope"));
static DB_SCOPE_WRITE: Lazy<ServiceScope> =
    Lazy::new(|| ServiceScope::new("db:write").expect("db:write scope"));

#[derive(Debug, Clone)]
pub enum DbConnectorEndpoint {
    #[cfg(unix)]
    Ipc {
        path: PathBuf,
    },
    Tcp {
        addr: SocketAddr,
    },
}

impl DbConnectorEndpoint {
    pub fn protocol(&self) -> &'static str {
        match self {
            #[cfg(unix)]
            DbConnectorEndpoint::Ipc { .. } => "ipc",
            DbConnectorEndpoint::Tcp { .. } => "tcp",
        }
    }

    pub fn location(&self) -> String {
        match self {
            #[cfg(unix)]
            DbConnectorEndpoint::Ipc { path } => path.display().to_string(),
            DbConnectorEndpoint::Tcp { addr } => addr.to_string(),
        }
    }

    pub fn uri(&self) -> String {
        match self {
            #[cfg(unix)]
            DbConnectorEndpoint::Ipc { path } => format!("ipc://{}", path.display()),
            DbConnectorEndpoint::Tcp { addr } => format!("tcp://{addr}"),
        }
    }
}

#[derive(Clone)]
pub struct DbConnectorService {
    db_shell: Arc<DbShellService>,
    security: Arc<SecurityManager>,
    diagnostics: Arc<ServiceDiagnostics>,
}

impl DbConnectorService {
    pub fn new(
        db_shell: Arc<DbShellService>,
        security: Arc<SecurityManager>,
        diagnostics: Arc<ServiceDiagnostics>,
    ) -> Self {
        Self {
            db_shell,
            security,
            diagnostics,
        }
    }

    pub async fn execute(&self, request: DbConnectorRequest) -> DbConnectorResponse {
        let started_at = Instant::now();
        let result = self.process(request).await;
        let success = result.is_ok();
        let response = match result {
            Ok(results) => DbConnectorResponse::ok(results.payload),
            Err(err) => {
                warn!(error = %err, "{}", db_connector_logs::REQUEST_FAILED);
                DbConnectorResponse::err(err.to_string())
            }
        };
        self.record_metrics(started_at, success);
        response
    }

    async fn process(
        &self,
        request: DbConnectorRequest,
    ) -> Result<DbConnectorResult, DbConnectorError> {
        let statement = request.command.statement();
        if statement.trim().is_empty() {
            return Err(DbConnectorError::InvalidRequest(
                db_connector_errors::STATEMENT_EMPTY.to_string(),
            ));
        }
        if statement.len() > MAX_STATEMENT_LEN {
            return Err(DbConnectorError::InvalidRequest(
                db_connector_errors::statement_too_long(MAX_STATEMENT_LEN),
            ));
        }
        let claims = self
            .security
            .validate_service_token(&request.token)
            .map_err(DbConnectorError::Token)?;
        match &claims.actor {
            DelegatedActor::Service { .. } => {}
            DelegatedActor::User { .. } => {
                return Err(DbConnectorError::Unauthorized(
                    db_connector_errors::actors_service_only().to_string(),
                ))
            }
        };
        let detected_intent = DbConnectorIntent::detect(statement);
        let exec_intent = request.intent.unwrap_or(detected_intent);
        let required_scope = match detected_intent {
            DbConnectorIntent::Read => DB_SCOPE_READ.as_str(),
            DbConnectorIntent::Write => DB_SCOPE_WRITE.as_str(),
        };
        if !self.claims_have_scope(&claims, required_scope) {
            return Err(DbConnectorError::Unauthorized(
                db_connector_errors::missing_scope(required_scope),
            ));
        }

        let mut session = self.db_shell.create_session();
        if let Some(engine_raw) = request.engine.as_deref() {
            let engine = DbEngine::from_str(engine_raw)
                .map_err(|err| DbConnectorError::InvalidRequest(err.to_string()))?;
            session
                .switch_engine(engine)
                .map_err(|err| DbConnectorError::InvalidRequest(err.to_string()))?;
        }

        let command = self.prepare_command(request.command, request.tenant, &claims.tenant_id)?;
        let payload = self
            .execute_command(&mut session, command, exec_intent)
            .await?;
        Ok(DbConnectorResult { payload })
    }

    fn record_metrics(&self, started_at: Instant, success: bool) {
        let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        self.diagnostics
            .record_probe("db-connector", latency_ms, success);
        self.diagnostics.record_heartbeat("db-connector");
    }

    fn claims_have_scope(&self, claims: &DelegatedTokenClaims, required: &str) -> bool {
        claims.scopes.iter().any(|scope| scope.as_str() == required)
    }

    async fn execute_command(
        &self,
        session: &mut crate::services::db_shell::DbShellSession,
        command: DbConnectorCommand,
        intent: DbConnectorIntent,
    ) -> Result<Vec<DbConnectorResultView>, DbConnectorError> {
        match command {
            DbConnectorCommand::Simple { statement } => {
                let results = session
                    .simple_query(&statement)
                    .await
                    .map_err(DbConnectorError::Db)?;
                Ok(results
                    .into_iter()
                    .map(DbConnectorResultView::from)
                    .collect())
            }
            DbConnectorCommand::Prepared { statement, params } => {
                let values = Self::convert_prepared_params(params)?;
                if matches!(intent, DbConnectorIntent::Write) {
                    let affected = session
                        .prepared_execute(&statement, &values)
                        .await
                        .map_err(DbConnectorError::Db)?;
                    Ok(vec![DbConnectorResultView::AffectedRows {
                        count: affected,
                    }])
                } else {
                    let results = session
                        .prepared_query(&statement, &values)
                        .await
                        .map_err(DbConnectorError::Db)?;
                    Ok(results
                        .into_iter()
                        .map(DbConnectorResultView::from)
                        .collect())
                }
            }
        }
    }

    fn prepare_command(
        &self,
        mut command: DbConnectorCommand,
        tenant_policy: Option<DbTenantPolicy>,
        tenant_id: &str,
    ) -> Result<DbConnectorCommand, DbConnectorError> {
        if let Some(policy) = tenant_policy {
            match &mut command {
                DbConnectorCommand::Prepared { params, .. } => {
                    Self::apply_tenant_policy(params, policy, tenant_id)?;
                }
                DbConnectorCommand::Simple { .. } => {
                    return Err(DbConnectorError::InvalidRequest(
                        db_connector_errors::tenant_policy_requires_prepared().to_string(),
                    ));
                }
            }
        }
        Ok(command)
    }

    fn apply_tenant_policy(
        params: &mut Vec<DbPreparedParam>,
        policy: DbTenantPolicy,
        tenant_id: &str,
    ) -> Result<(), DbConnectorError> {
        let DbTenantPolicy { param, mode } = policy;
        match mode {
            DbTenantBindingMode::Inject => {
                let tenant_value = JsonValue::String(tenant_id.to_string());
                if let Some(target) = params.iter_mut().find(|p| p.name == param) {
                    target.value = tenant_value;
                } else {
                    params.push(DbPreparedParam {
                        name: param,
                        value: tenant_value,
                    });
                }
                Ok(())
            }
            DbTenantBindingMode::RequireMatch => {
                let Some(target) = params.iter().find(|p| p.name == param) else {
                    return Err(DbConnectorError::InvalidRequest(
                        db_connector_errors::tenant_param_missing(&param),
                    ));
                };
                match &target.value {
                    JsonValue::String(value) if value == tenant_id => Ok(()),
                    _ => Err(DbConnectorError::InvalidRequest(
                        db_connector_errors::tenant_param_mismatch(&param),
                    )),
                }
            }
        }
    }

    fn convert_prepared_params(
        params: Vec<DbPreparedParam>,
    ) -> Result<Vec<DbValue>, DbConnectorError> {
        params
            .into_iter()
            .map(|param| Self::parse_param_value_with_name(&param.name, param.value))
            .collect()
    }

    fn parse_param_value_with_name(
        name: &str,
        value: JsonValue,
    ) -> Result<DbValue, DbConnectorError> {
        match value {
            JsonValue::Null => {
                // Detect column type by name convention
                if Self::looks_like_timestamp_column(name) {
                    Ok(DbValue::NullTimestamp)
                } else if Self::looks_like_uuid_column(name) {
                    Ok(DbValue::NullUuid)
                } else {
                    // Default to generic NULL (works for TEXT, VARCHAR, etc.)
                    // INET columns are rare - treat IP addresses as text by default
                    Ok(DbValue::Null)
                }
            }
            JsonValue::Bool(flag) => Ok(DbValue::Bool(flag)),
            JsonValue::Number(num) => {
                if let Some(int) = num.as_i64() {
                    if Self::looks_like_bool_column(name) {
                        return match int {
                            0 => Ok(DbValue::Bool(false)),
                            1 => Ok(DbValue::Bool(true)),
                            _ => Err(DbConnectorError::InvalidRequest(format!(
                                "invalid boolean value for {name}: {int}",
                            ))),
                        };
                    }
                    // Always use i64 - Postgres will accept it for both INT4 and INT8 columns
                    // The database will handle overflow errors if the value is too large for INT4
                    Ok(DbValue::Integer(int))
                } else if let Some(float) = num.as_f64() {
                    Ok(DbValue::Float(float))
                } else {
                    Err(DbConnectorError::InvalidRequest(
                        "unsupported number".to_string(),
                    ))
                }
            }
            JsonValue::String(text) => {
                if Self::looks_like_text_array_column(name) {
                    return Ok(Self::parse_text_array_value(&text));
                }
                // Try to detect UUID strings (e.g., "550e8400-e29b-41d4-a716-446655440000")
                if Self::looks_like_uuid(&text) {
                    if let Ok(uuid) = uuid::Uuid::parse_str(&text) {
                        return Ok(DbValue::Uuid(uuid));
                    }
                }
                // Try to detect ISO 8601 timestamps (e.g., "2024-01-01T12:00:00Z" or with timezone)
                if Self::looks_like_timestamp(&text) {
                    // Try to parse as RFC 3339 timestamp
                    if let Ok(ts) = time::OffsetDateTime::parse(
                        &text,
                        &time::format_description::well_known::Rfc3339,
                    ) {
                        return Ok(DbValue::Timestamp(ts));
                    }
                    // Fall back to timestamp string for Postgres to parse
                    return Ok(DbValue::TimestampStr(text));
                }
                // Default: treat as text (works for VARCHAR, TEXT, and IP addresses stored as text)
                Ok(DbValue::Text(text))
            }
            JsonValue::Array(values) => {
                if Self::looks_like_text_array_column(name) {
                    Ok(Self::parse_text_array_items(values))
                } else {
                    serde_json::to_string(&JsonValue::Array(values))
                        .map(DbValue::Json)
                        .map_err(|err| DbConnectorError::InvalidRequest(err.to_string()))
                }
            }
            other => serde_json::to_string(&other)
                .map(DbValue::Json)
                .map_err(|err| DbConnectorError::InvalidRequest(err.to_string())),
        }
    }

    /// Heuristic to detect if a column name refers to a timestamp
    fn looks_like_timestamp_column(name: &str) -> bool {
        let lower = name.to_lowercase();
        lower.ends_with("_at")
            || lower.ends_with("_until")
            || lower.ends_with("_date")
            || lower.ends_with("_time")
            || lower == "created"
            || lower == "updated"
            || lower == "timestamp"
    }

    /// Heuristic to detect if a column stores a boolean flag
    fn looks_like_bool_column(name: &str) -> bool {
        let lower = name.to_lowercase();
        matches!(
            lower.as_str(),
            "enabled" | "requires_api_key" | "maintenance_mode" | "email_verified" | "success"
        ) || lower.starts_with("is_")
            || lower.starts_with("has_")
            || lower.ends_with("_enabled")
            || lower.ends_with("_active")
            || lower.ends_with("_verified")
            || lower.ends_with("_required")
    }

    /// Heuristic to detect if a column stores a text array
    fn looks_like_text_array_column(name: &str) -> bool {
        let lower = name.to_lowercase();
        matches!(lower.as_str(), "allowed_email_domains" | "scopes")
            || lower.ends_with("_domains")
            || lower.ends_with("_scopes")
    }

    fn parse_text_array_items(values: Vec<JsonValue>) -> DbValue {
        let items = values
            .into_iter()
            .filter_map(|value| match value {
                JsonValue::String(text) => Some(text),
                JsonValue::Number(num) => Some(num.to_string()),
                JsonValue::Bool(flag) => Some(flag.to_string()),
                JsonValue::Null => None,
                other => Some(other.to_string()),
            })
            .collect::<Vec<_>>();
        DbValue::TextArray(items)
    }

    fn parse_text_array_value(text: &str) -> DbValue {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return DbValue::TextArray(Vec::new());
        }
        if let Ok(JsonValue::Array(values)) = serde_json::from_str::<JsonValue>(trimmed) {
            return Self::parse_text_array_items(values);
        }
        DbValue::TextArray(vec![text.to_string()])
    }

    /// Heuristic to detect if a column is a UUID
    fn looks_like_uuid_column(name: &str) -> bool {
        let lower = name.to_lowercase();
        lower.ends_with("_id") || lower == "id" || lower == "uuid" || lower.ends_with("_uuid")
    }

    /// Heuristic to detect if a string looks like a UUID
    fn looks_like_uuid(s: &str) -> bool {
        // Standard UUID format: 8-4-4-4-12 = 36 chars with hyphens
        if s.len() != 36 {
            return false;
        }
        let bytes = s.as_bytes();
        // Check for hyphens at expected positions
        bytes[8] == b'-' && bytes[13] == b'-' && bytes[18] == b'-' && bytes[23] == b'-'
    }

    /// Heuristic to detect if a string looks like an ISO 8601 timestamp
    fn looks_like_timestamp(s: &str) -> bool {
        // Must be at least "YYYY-MM-DDTHH:MM:SS" (19 chars)
        if s.len() < 19 {
            return false;
        }
        // Check for typical ISO 8601 patterns
        let bytes = s.as_bytes();
        // YYYY-MM-DDTHH:MM:SS
        bytes[4] == b'-'
            && bytes[7] == b'-'
            && (bytes[10] == b'T' || bytes[10] == b' ')
            && bytes[13] == b':'
            && bytes[16] == b':'
    }
}

#[derive(Debug)]
struct DbConnectorResult {
    payload: Vec<DbConnectorResultView>,
}

impl From<DbExecutionResult> for DbConnectorResultView {
    fn from(value: DbExecutionResult) -> Self {
        match value {
            DbExecutionResult::ResultSet(set) => Self::ResultSet {
                columns: set.columns,
                rows: set.rows,
            },
            DbExecutionResult::AffectedRows(count) => Self::AffectedRows { count },
            DbExecutionResult::CommandTag(tag) => Self::Command { tag },
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum DbConnectorError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("database error: {0}")]
    Db(#[from] DbError),
    #[error("token validation failed: {0}")]
    Token(#[from] SecurityError),
}

impl From<ServiceScopeError> for DbConnectorError {
    fn from(err: ServiceScopeError) -> Self {
        Self::InvalidRequest(err.to_string())
    }
}
