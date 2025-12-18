use tracing::warn;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::domain::module::ModuleId;
use crate::security::manager::AuditSink;
use crate::security::service::ServiceScope;

pub const MODULE_SERVICE_TOKEN_EXCHANGE_ACTION: &str = "module::service-token-exchange";

pub struct ModuleTokenAuditContext<'a> {
    pub transport: &'a str,
    pub endpoint: Option<&'a str>,
    pub requested_scopes: &'a [String],
    pub granted_scopes: Option<&'a [ServiceScope]>,
    pub reason: Option<&'a str>,
    pub expires_in_seconds: u64,
    pub outcome: AuditOutcome,
    pub error: Option<&'a str>,
}

pub fn record_module_token_exchange(
    sink: &dyn AuditSink,
    module_id: &ModuleId,
    ctx: ModuleTokenAuditContext<'_>,
) {
    let endpoint = ctx.endpoint.unwrap_or("-");
    let mut metadata = AuditMetadata::default()
        .insert("transport", ctx.transport)
        .insert("endpoint", endpoint)
        .insert(
            "requested_scopes",
            format_requested_scope_list(ctx.requested_scopes),
        )
        .insert(
            "granted_scopes",
            ctx.granted_scopes
                .map(format_granted_scope_list)
                .unwrap_or_else(|| "-".to_string()),
        )
        .insert("expires_in_seconds", ctx.expires_in_seconds.to_string());
    if let Some(reason) = ctx.reason {
        metadata = metadata.insert("reason", reason);
    }
    if let Some(err_msg) = ctx.error {
        metadata = metadata.insert("error", err_msg);
    }
    let event = AuditEvent::builder()
        .actor(AuditActor::System)
        .action(MODULE_SERVICE_TOKEN_EXCHANGE_ACTION.to_string())
        .target(format!("module:{}", module_id.as_str()))
        .outcome(ctx.outcome)
        .metadata(metadata)
        .build();
    match event {
        Ok(event) => {
            if let Err(err) = sink.record(event) {
                warn!(error = %err, "module token exchange audit append failed");
            }
        }
        Err(err) => warn!(
            error = %err,
            "module token exchange audit build failed"
        ),
    }
}

fn format_requested_scope_list(scopes: &[String]) -> String {
    if scopes.is_empty() {
        return "-".to_string();
    }
    scopes
        .iter()
        .map(|scope| scope.trim())
        .filter(|scope| !scope.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

fn format_granted_scope_list(scopes: &[ServiceScope]) -> String {
    if scopes.is_empty() {
        return "-".to_string();
    }
    scopes
        .iter()
        .map(|scope| scope.as_str())
        .collect::<Vec<_>>()
        .join(",")
}
