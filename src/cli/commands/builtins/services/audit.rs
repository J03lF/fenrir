use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::CliDependencies;
use crate::services::{ServiceControlError, ServiceControlOutcome};
use tracing::warn;

const TRANSPORT: &str = "cli";

pub(super) fn record_cli_service_action(
    deps: &CliDependencies,
    action: &str,
    target: &str,
    force: bool,
    result: &Result<ServiceControlOutcome, ServiceControlError>,
) {
    let actor = cli_actor(deps);
    let mut metadata = base_metadata(action, force);
    let outcome = match result {
        Ok(control) => {
            metadata = metadata.insert("service_outcome", control.as_str());
            AuditOutcome::Success
        }
        Err(err) => {
            metadata = metadata
                .insert("error_code", control_error_code(err))
                .insert("message", control_error_message(err));
            AuditOutcome::Failure
        }
    };
    push_audit_event(deps, actor, action, target, outcome, metadata);
}

pub(super) fn record_cli_bulk_action(
    deps: &CliDependencies,
    action: &str,
    force: bool,
    success: usize,
    failure_count: usize,
    failures: Vec<(String, String)>,
) {
    let actor = cli_actor(deps);
    let mut metadata = base_metadata(action, force)
        .insert("mode", "bulk")
        .insert("success", success.to_string())
        .insert("failed", failure_count.to_string());
    if failure_count > 0 {
        let ids: Vec<String> = failures.iter().map(|(id, _)| id.clone()).collect();
        let messages: Vec<String> = failures.iter().map(|(_, msg)| msg.clone()).collect();
        metadata = metadata
            .insert("failed_ids", ids.join(","))
            .insert("errors", messages.join(" | "));
    }
    let outcome = if failure_count == 0 {
        AuditOutcome::Success
    } else {
        AuditOutcome::Failure
    };
    push_audit_event(deps, actor, action, "--all", outcome, metadata);
}

fn cli_actor(deps: &CliDependencies) -> AuditActor {
    if let Some(actor) = deps.session_actor() {
        return actor.clone();
    }
    AuditActor::User {
        user_id: format!("cli::{}", whoami::username()),
        role: "operator".to_string(),
    }
}

fn base_metadata(action: &str, force: bool) -> AuditMetadata {
    let host = whoami::fallible::hostname().unwrap_or_else(|_| "unknown-host".to_string());
    AuditMetadata::default()
        .insert("transport", TRANSPORT)
        .insert("command", format!("{action} service"))
        .insert("force", if force { "true" } else { "false" })
        .insert("host", host)
}

fn push_audit_event(
    deps: &CliDependencies,
    actor: AuditActor,
    action: &str,
    target: &str,
    outcome: AuditOutcome,
    metadata: AuditMetadata,
) {
    match AuditEvent::builder()
        .actor(actor)
        .action(format!("service::{action}"))
        .target(target.to_string())
        .outcome(outcome)
        .metadata(metadata)
        .build()
    {
        Ok(event) => {
            if let Err(err) = deps.services.record_audit(event) {
                warn!(error = %err, "cli audit append failed");
            }
        }
        Err(err) => warn!(error = %err, "cli audit build failed"),
    }
}

fn control_error_code(err: &ServiceControlError) -> &'static str {
    match err {
        ServiceControlError::UnknownService(_) => "unknown_service",
        ServiceControlError::NotControllable(_) => "not_controllable",
        ServiceControlError::ForceRequired(_) => "force_required",
        ServiceControlError::CoreLocked(_) => "core_locked",
        ServiceControlError::OperationFailed { .. } => "operation_failed",
    }
}

fn control_error_message(err: &ServiceControlError) -> String {
    match err {
        ServiceControlError::UnknownService(_) => "Service ist nicht registriert".to_string(),
        ServiceControlError::NotControllable(_) => {
            "Service erlaubt keine Steuerung über CLI".to_string()
        }
        ServiceControlError::ForceRequired(_) => {
            "Aktion erfordert --force für kritischen Service".to_string()
        }
        ServiceControlError::CoreLocked(_) => "Core-Service blockiert für Stop/Restart".to_string(),
        ServiceControlError::OperationFailed { source, .. } => source.to_string(),
    }
}
