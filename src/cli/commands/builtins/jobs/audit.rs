use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::CliDependencies;
use tracing::warn;

const TRANSPORT: &str = "cli";

pub(super) fn record_job_action(
    deps: &CliDependencies,
    action: &str,
    job_id: &str,
    outcome: AuditOutcome,
    metadata: AuditMetadata,
) {
    let actor = cli_actor(deps);
    match AuditEvent::builder()
        .actor(actor)
        .action(format!("job::{action}"))
        .target(job_id.to_string())
        .outcome(outcome)
        .metadata(metadata)
        .build()
    {
        Ok(event) => {
            if let Err(err) = deps.services.record_audit(event) {
                warn!(error = %err, "cli job audit append failed");
            }
        }
        Err(err) => warn!(error = %err, "cli job audit build failed"),
    }
}

pub(super) fn base_metadata(action: &str) -> AuditMetadata {
    let host = whoami::fallible::hostname().unwrap_or_else(|_| "unknown-host".to_string());
    AuditMetadata::default()
        .insert("transport", TRANSPORT)
        .insert("command", format!("job {action}"))
        .insert("host", host)
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
