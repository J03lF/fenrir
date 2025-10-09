use std::io::{self, Write};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::audit::{AuditActor, AuditEvent, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, CommandShape, ShellEnvironment,
};

const DETAILS: &[&str] = &[
    "audit                      – zeigt die letzten 20 Audit-Einträge",
    "audit --limit <n>          – begrenzt die Anzahl der Einträge",
    "audit --action <code>      – filtert nach Aktions-Code",
    "audit --outcome <status>   – filtert nach Ergebnis (success|failure|denied)",
    "audit --actor system       – nur Systemereignisse anzeigen",
];

const AUDIT_SHAPE: CommandShape = CommandShape::basic("audit");

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "audit",
        "Listet aktuelle Audit-Events (nur lesend)",
        "audit [--limit <n>] [--action <code>] [--outcome <status>] [--actor <id>]",
        DETAILS,
        handle,
        AUDIT_SHAPE,
    )
}

fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    if !deps.config.audit.enabled {
        writeln!(
            out,
            "Audit-Logging ist laut Konfiguration deaktiviert. Aktivieren via [audit.enabled].",
        )?;
        return Ok(CommandOutcome::Continue);
    }

    let mut limit = 20_usize;
    let mut action_filter: Option<&str> = None;
    let mut outcome_filter: Option<&str> = None;
    let mut actor_filter: Option<&str> = None;

    let mut idx = 0;
    while idx < args.len() {
        match args[idx] {
            "--limit" => {
                idx += 1;
                let Some(value) = args.get(idx) else {
                    writeln!(out, "Fehlender Wert nach --limit")?;
                    return Ok(CommandOutcome::Continue);
                };
                match value.parse::<usize>() {
                    Ok(parsed) if parsed > 0 => limit = parsed.min(500),
                    _ => {
                        writeln!(out, "Ungültiger --limit Wert. Erlaubt: positive Ganzzahlen")?;
                        return Ok(CommandOutcome::Continue);
                    }
                }
            }
            "--action" => {
                idx += 1;
                action_filter = args.get(idx).copied();
            }
            "--outcome" => {
                idx += 1;
                outcome_filter = args.get(idx).copied();
            }
            "--actor" => {
                idx += 1;
                actor_filter = args.get(idx).copied();
            }
            other => {
                writeln!(out, "Unbekannter Parameter: {other}")?;
                return Ok(CommandOutcome::Continue);
            }
        }
        idx += 1;
    }

    if limit == 0 {
        writeln!(out, "Limit 0 liefert keine Ergebnisse.")?;
        return Ok(CommandOutcome::Continue);
    }

    let events = match deps.services.audit_recent(limit) {
        Ok(events) => events,
        Err(err) => {
            writeln!(out, "Konnte Audit-Events nicht laden: {err}")?;
            return Ok(CommandOutcome::Continue);
        }
    };

    if events.is_empty() {
        writeln!(out, "Keine Audit-Ereignisse vorhanden.")?;
        return Ok(CommandOutcome::Continue);
    }

    writeln!(out, "Audit-Events (neueste zuerst, max. {limit}):")?;

    let mut shown = 0usize;

    for event in events {
        if let Some(filter) = action_filter {
            if event.action != filter {
                continue;
            }
        }
        if let Some(filter) = outcome_filter {
            if !outcome_matches(&event.outcome, filter) {
                continue;
            }
        }
        if let Some(filter) = actor_filter {
            if !actor_matches(&event.actor, filter) {
                continue;
            }
        }

        render_event(out, &event)?;
        shown += 1;
    }

    if shown == 0 {
        writeln!(
            out,
            "Keine Audit-Ereignisse entsprechen den gesetzten Filtern."
        )?;
    }

    Ok(CommandOutcome::Continue)
}

fn render_event(out: &mut dyn Write, event: &AuditEvent) -> io::Result<()> {
    let timestamp = OffsetDateTime::from(event.timestamp);
    let ts = timestamp
        .format(&Rfc3339)
        .unwrap_or_else(|_| timestamp.to_string());

    let actor = describe_actor(&event.actor, &event.redactions);
    let outcome = event.outcome.to_string();
    writeln!(
        out,
        "{ts} | actor={actor} | action={} | target={} | outcome={outcome}",
        event.action, event.target
    )?;

    let metadata = format_metadata(event);
    if !metadata.is_empty() {
        writeln!(out, "  meta: {metadata}")?;
    }

    if !event.redactions.is_empty() {
        writeln!(out, "  redactions: {}", event.redactions.join(", "))?;
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
                format!("{key}=<redacted>")
            } else {
                format!("{key}={value}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe_actor(actor: &AuditActor, redactions: &[String]) -> String {
    match actor {
        AuditActor::System => "system".to_string(),
        AuditActor::User { user_id, role } => {
            let hide_id = redactions.iter().any(|field| field == "user_id");
            if hide_id {
                format!("user(role={role})")
            } else {
                format!("user(id={user_id}, role={role})")
            }
        }
    }
}

fn outcome_matches(outcome: &AuditOutcome, filter: &str) -> bool {
    let normalized = filter.to_ascii_lowercase();
    match outcome {
        AuditOutcome::Success => normalized == "success",
        AuditOutcome::Failure => normalized == "failure",
        AuditOutcome::Denied => normalized == "denied",
    }
}

fn actor_matches(actor: &AuditActor, filter: &str) -> bool {
    match actor {
        AuditActor::System => filter.eq_ignore_ascii_case("system"),
        AuditActor::User { user_id, role } => {
            filter.eq_ignore_ascii_case(role) || user_id.eq_ignore_ascii_case(filter)
        }
    }
}
