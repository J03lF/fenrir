use std::io::{self, Write};

use crate::audit::AuditOutcome;
use crate::cli::commands::registry::CliDependencies;
use crate::services::scheduler::JobControlOutcome;
use crate::services::JobLogError;
use crate::utils::messages::cli::builtins::jobs as job_messages;

use super::audit::{base_metadata, record_job_action};
use super::view::{job_status_label, render_job_logs, render_job_status};

pub const DEFAULT_LOG_TAIL: usize = 40;

pub fn show_job_status(
    deps: &CliDependencies,
    job_id: &str,
    out: &mut dyn Write,
) -> io::Result<()> {
    match deps.services.scheduler_job(job_id) {
        Some(snapshot) => render_job_status(&snapshot, out),
        None => {
            writeln!(out, "{}", job_messages::status::unknown_job(job_id))?;
            Ok(())
        }
    }
}

pub fn stream_job_logs(
    deps: &CliDependencies,
    job_id: &str,
    tail: usize,
    out: &mut dyn Write,
) -> io::Result<()> {
    match deps.services.job_logs(job_id, tail) {
        Ok(snapshot) => render_job_logs(snapshot, out, tail),
        Err(JobLogError::LogUnavailable) => {
            writeln!(out, "{}", job_messages::logs::LOG_UNAVAILABLE)
        }
        Err(JobLogError::Io(err)) => {
            writeln!(out, "{}", job_messages::logs::io_error(&err))
        }
    }
}

pub fn restart_job_cli(
    deps: &CliDependencies,
    job_id: &str,
    out: &mut dyn Write,
) -> io::Result<()> {
    let previous_state = scheduler_state_label(deps, job_id);
    let result = deps.services.restart_job(job_id);
    let mut metadata = base_metadata("restart")
        .insert("job_id", job_id)
        .insert("previous_state", previous_state.clone());
    match result {
        Ok(JobControlOutcome::Restarted) => {
            writeln!(out, "{}", job_messages::restart::success(job_id))?;
            metadata = metadata.insert("outcome", JobControlOutcome::Restarted.as_str());
            record_job_action(deps, "restart", job_id, AuditOutcome::Success, metadata);
        }
        Ok(JobControlOutcome::SchedulerInactive) => {
            writeln!(out, "{}", job_messages::restart::scheduler_inactive())?;
            metadata = metadata.insert("outcome", JobControlOutcome::SchedulerInactive.as_str());
            record_job_action(
                deps,
                "restart",
                job_id,
                AuditOutcome::Failure,
                metadata.insert("error_code", "scheduler_inactive"),
            );
        }
        Ok(JobControlOutcome::Paused) => {
            writeln!(out, "{}", job_messages::restart::job_paused(job_id))?;
            metadata = metadata.insert("outcome", JobControlOutcome::Paused.as_str());
            record_job_action(
                deps,
                "restart",
                job_id,
                AuditOutcome::Failure,
                metadata.insert("error_code", "job_paused"),
            );
        }
        Ok(other) => {
            writeln!(
                out,
                "{}",
                job_messages::restart::unexpected_outcome(other.as_str())
            )?;
            metadata = metadata.insert("outcome", other.as_str());
            record_job_action(
                deps,
                "restart",
                job_id,
                AuditOutcome::Failure,
                metadata.insert("error_code", "unexpected_outcome"),
            );
        }
        Err(err) => {
            writeln!(out, "{}", err)?;
            record_job_action(
                deps,
                "restart",
                job_id,
                AuditOutcome::Failure,
                metadata
                    .insert("outcome", "error")
                    .insert("error_code", err.to_string()),
            );
        }
    }
    Ok(())
}

pub fn pause_job_cli(deps: &CliDependencies, job_id: &str, out: &mut dyn Write) -> io::Result<()> {
    let previous_state = scheduler_state_label(deps, job_id);
    let mut metadata = base_metadata("pause")
        .insert("job_id", job_id)
        .insert("previous_state", previous_state.clone());

    match deps.services.pause_job(job_id) {
        Ok(JobControlOutcome::Paused) => {
            writeln!(out, "{}", job_messages::pause::success(job_id))?;
            metadata = metadata.insert("outcome", JobControlOutcome::Paused.as_str());
            record_job_action(deps, "pause", job_id, AuditOutcome::Success, metadata);
        }
        Ok(JobControlOutcome::AlreadyPaused) => {
            writeln!(out, "{}", job_messages::pause::already_paused(job_id))?;
            metadata = metadata.insert("outcome", JobControlOutcome::AlreadyPaused.as_str());
            record_job_action(deps, "pause", job_id, AuditOutcome::Success, metadata);
        }
        Ok(JobControlOutcome::SchedulerInactive) => {
            writeln!(out, "{}", job_messages::pause::scheduler_inactive())?;
            metadata = metadata.insert("outcome", JobControlOutcome::SchedulerInactive.as_str());
            record_job_action(
                deps,
                "pause",
                job_id,
                AuditOutcome::Failure,
                metadata.insert("error_code", "scheduler_inactive"),
            );
        }
        Ok(other) => {
            writeln!(
                out,
                "{}",
                job_messages::pause::unexpected_outcome(other.as_str())
            )?;
            metadata = metadata.insert("outcome", other.as_str());
            record_job_action(
                deps,
                "pause",
                job_id,
                AuditOutcome::Failure,
                metadata.insert("error_code", "unexpected_outcome"),
            );
        }
        Err(err) => {
            writeln!(out, "{}", err)?;
            record_job_action(
                deps,
                "pause",
                job_id,
                AuditOutcome::Failure,
                metadata
                    .insert("outcome", "error")
                    .insert("error_code", err.to_string()),
            );
        }
    }
    Ok(())
}

pub fn resume_job_cli(deps: &CliDependencies, job_id: &str, out: &mut dyn Write) -> io::Result<()> {
    let previous_state = scheduler_state_label(deps, job_id);
    let mut metadata = base_metadata("resume")
        .insert("job_id", job_id)
        .insert("previous_state", previous_state.clone());

    match deps.services.resume_job(job_id) {
        Ok(JobControlOutcome::Resumed) => {
            writeln!(out, "{}", job_messages::resume::success(job_id))?;
            metadata = metadata.insert("outcome", JobControlOutcome::Resumed.as_str());
            record_job_action(deps, "resume", job_id, AuditOutcome::Success, metadata);
        }
        Ok(JobControlOutcome::AlreadyActive) => {
            writeln!(out, "{}", job_messages::resume::already_active(job_id))?;
            metadata = metadata.insert("outcome", JobControlOutcome::AlreadyActive.as_str());
            record_job_action(deps, "resume", job_id, AuditOutcome::Success, metadata);
        }
        Ok(JobControlOutcome::SchedulerInactive) => {
            writeln!(out, "{}", job_messages::resume::scheduler_inactive())?;
            metadata = metadata.insert("outcome", JobControlOutcome::SchedulerInactive.as_str());
            record_job_action(
                deps,
                "resume",
                job_id,
                AuditOutcome::Failure,
                metadata.insert("error_code", "scheduler_inactive"),
            );
        }
        Ok(other) => {
            writeln!(
                out,
                "{}",
                job_messages::resume::unexpected_outcome(other.as_str())
            )?;
            metadata = metadata.insert("outcome", other.as_str());
            record_job_action(
                deps,
                "resume",
                job_id,
                AuditOutcome::Failure,
                metadata.insert("error_code", "unexpected_outcome"),
            );
        }
        Err(err) => {
            writeln!(out, "{}", err)?;
            record_job_action(
                deps,
                "resume",
                job_id,
                AuditOutcome::Failure,
                metadata
                    .insert("outcome", "error")
                    .insert("error_code", err.to_string()),
            );
        }
    }
    Ok(())
}

pub fn parse_tail_flag(args: &[&str]) -> Result<usize, String> {
    if args.is_empty() {
        return Ok(DEFAULT_LOG_TAIL);
    }

    let mut iter = args.iter().copied();
    let mut tail = DEFAULT_LOG_TAIL;
    while let Some(token) = iter.next() {
        match token {
            "--tail" => {
                let Some(value) = iter.next() else {
                    return Err(job_messages::logs::MISSING_TAIL_VALUE.to_string());
                };
                tail = value
                    .parse::<usize>()
                    .map_err(|_| job_messages::logs::invalid_tail(value))?;
                if tail == 0 {
                    return Err(job_messages::logs::TAIL_MINIMUM.to_string());
                }
            }
            other => return Err(job_messages::logs::unexpected_argument(other)),
        }
    }
    Ok(tail)
}

fn scheduler_state_label(deps: &CliDependencies, job_id: &str) -> String {
    deps.services
        .scheduler_job(job_id)
        .map(|snapshot| job_status_label(&snapshot).to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
