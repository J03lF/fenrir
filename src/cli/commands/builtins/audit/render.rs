use crate::audit::{AuditActor, AuditEvent};
use crate::utils::messages::audit::actors;
use crate::utils::messages::cli::builtins::audit::render as audit_render_messages;
use crate::utils::system_time_to_rfc3339;
use std::io::{self, Write};

pub fn render_event(out: &mut dyn Write, event: &AuditEvent) -> io::Result<()> {
    let ts = system_time_to_rfc3339(event.timestamp)
        .unwrap_or_else(|| audit_render_messages::INVALID_TIMESTAMP.to_string());

    let actor = describe_actor(&event.actor, &event.redactions);
    let outcome = event.outcome.to_string();
    writeln!(
        out,
        "{}",
        audit_render_messages::event_line(&ts, &actor, &event.action, &event.target, &outcome)
    )?;

    let metadata = format_metadata(event);
    if !metadata.is_empty() {
        writeln!(out, "{}{metadata}", audit_render_messages::META_PREFIX)?;
    }

    if !event.redactions.is_empty() {
        writeln!(
            out,
            "{}{}",
            audit_render_messages::REDACTIONS_PREFIX,
            event.redactions.join(", ")
        )?;
    }

    Ok(())
}

fn format_metadata(event: &AuditEvent) -> String {
    if event.metadata.as_slice().is_empty() {
        return String::new();
    }
    event
        .metadata
        .as_slice()
        .iter()
        .map(|(key, value)| {
            if event.redactions.iter().any(|field| field == key) {
                audit_render_messages::format_redacted_entry(key)
            } else {
                audit_render_messages::format_meta_entry(key, value)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe_actor(actor: &AuditActor, redactions: &[String]) -> String {
    match actor {
        AuditActor::System => actors::SYSTEM.to_string(),
        AuditActor::User { user_id, role } => {
            let hide_id = redactions.iter().any(|field| field == "user_id");
            if hide_id {
                audit_render_messages::actor_user_with_role(role)
            } else {
                audit_render_messages::actor_user_with_id(user_id, role)
            }
        }
    }
}
