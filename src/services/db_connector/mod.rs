use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use once_cell::sync::Lazy;
use serde_json::Value as JsonValue;
use tracing::{info, warn};

use crate::domain::db::{DbEngine, DbError, DbExecutionResult, DbValue};
use crate::security::manager::{SecurityError, SecurityManager};
use crate::security::service::{ServiceScope, ServiceScopeError};
use crate::security::service_tokens::{DelegatedActor, DelegatedTokenClaims};
use crate::services::db_shell::DbShellService;
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
}

impl DbConnectorService {
    pub fn new(db_shell: Arc<DbShellService>, security: Arc<SecurityManager>) -> Self {
        Self { db_shell, security }
    }

    pub async fn execute(&self, request: DbConnectorRequest) -> DbConnectorResponse {
        match self.process(request).await {
            Ok(results) => {
                info!(
                    tenant = results.tenant_id.as_str(),
                    "{}",
                    db_connector_logs::REQUEST_OK
                );
                DbConnectorResponse::ok(results.payload)
            }
            Err(err) => {
                warn!(error = %err, "{}", db_connector_logs::REQUEST_FAILED);
                DbConnectorResponse::err(err.to_string())
            }
        }
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
        Ok(DbConnectorResult {
            tenant_id: claims.tenant_id.clone(),
            payload,
        })
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
            .map(|param| Self::parse_param_value(param.value))
            .collect()
    }

    fn parse_param_value(value: JsonValue) -> Result<DbValue, DbConnectorError> {
        match value {
            JsonValue::Null => Ok(DbValue::Null),
            JsonValue::Bool(flag) => Ok(DbValue::Bool(flag)),
            JsonValue::Number(num) => {
                if let Some(int) = num.as_i64() {
                    Ok(DbValue::Integer(int))
                } else if let Some(float) = num.as_f64() {
                    Ok(DbValue::Float(float))
                } else {
                    Err(DbConnectorError::InvalidRequest(
                        "unsupported number".to_string(),
                    ))
                }
            }
            JsonValue::String(text) => Ok(DbValue::Text(text)),
            other => serde_json::to_string(&other)
                .map(DbValue::Json)
                .map_err(|err| DbConnectorError::InvalidRequest(err.to_string())),
        }
    }
}

#[derive(Debug)]
struct DbConnectorResult {
    tenant_id: String,
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
