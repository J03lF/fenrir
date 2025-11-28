use std::io::{self, Write};

use crate::audit::{AuditActor, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, CommandShape, ShellEnvironment,
};

use super::render::render_event;
use crate::utils::messages::cli::builtins::audit::{
    command as audit_command_messages, responses as audit_responses,
};

const AUDIT_SHAPE: CommandShape = CommandShape::basic("audit");

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        audit_command_messages::NAME,
        audit_command_messages::DESCRIPTION,
        audit_command_messages::USAGE,
        audit_command_messages::DETAILS,
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
        writeln!(out, "{}", audit_responses::DISABLED)?;
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
                    writeln!(out, "{}", audit_responses::MISSING_LIMIT_VALUE)?;
                    return Ok(CommandOutcome::Continue);
                };
                match value.parse::<usize>() {
                    Ok(parsed) if parsed > 0 => limit = parsed.min(500),
                    _ => {
                        writeln!(out, "{}", audit_responses::INVALID_LIMIT)?;
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
                writeln!(out, "{}", audit_responses::unknown_parameter(other))?;
                return Ok(CommandOutcome::Continue);
            }
        }
        idx += 1;
    }

    if limit == 0 {
        writeln!(out, "{}", audit_responses::ZERO_LIMIT_WARNING)?;
        return Ok(CommandOutcome::Continue);
    }

    let events = match deps.services.audit_recent(limit) {
        Ok(events) => events,
        Err(err) => {
            writeln!(out, "{}", audit_responses::load_failed(&err))?;
            return Ok(CommandOutcome::Continue);
        }
    };

    if events.is_empty() {
        writeln!(out, "{}", audit_responses::NONE_AVAILABLE)?;
        return Ok(CommandOutcome::Continue);
    }

    writeln!(out, "{}", audit_responses::header(limit))?;

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
        writeln!(out, "{}", audit_responses::FILTER_NONE)?;
    }

    Ok(CommandOutcome::Continue)
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
