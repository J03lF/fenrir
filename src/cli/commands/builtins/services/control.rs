use super::audit::{record_cli_bulk_action, record_cli_service_action};
use super::metadata::metadata_for_action;
use crate::cli::commands::registry::CliDependencies;
use crate::services::{
    ServiceActionKind, ServiceActionReport, ServiceControlError, ServiceControlOutcome,
};
use crate::utils::messages::cli::builtins::services::control as control_messages;
use std::io::{self, Write};

pub(super) fn start_service(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    apply_service_action(ServiceActionKind::Start, deps, args, out)
}

pub(super) fn stop_service(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    apply_service_action(ServiceActionKind::Stop, deps, args, out)
}

pub(super) fn restart_service(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    apply_service_action(ServiceActionKind::Restart, deps, args, out)
}

fn apply_service_action(
    action: ServiceActionKind,
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    let parsed = ParsedServiceArgs::from(args);
    let force = if action.supports_force() {
        parsed.force()
    } else {
        false
    };

    if parsed.apply_all() {
        let reports = dispatch_bulk(action, deps, force);
        render_bulk_results(deps, out, action.verb(), force, reports)?;
        return Ok(());
    }

    let Some(id) = parsed.target() else {
        writeln!(
            out,
            "{}",
            control_messages::missing_service_id(metadata_for_action(action).usage())
        )?;
        return Ok(());
    };

    let result = dispatch_single(action, deps, id, force);
    record_cli_service_action(deps, action.verb(), id, force, &result);
    match result {
        Ok(outcome) => render_single_outcome(action, id, outcome, out)?,
        Err(err) => render_control_error(err, id, out)?,
    }
    Ok(())
}

fn dispatch_bulk(
    action: ServiceActionKind,
    deps: &CliDependencies,
    force: bool,
) -> Vec<ServiceActionReport> {
    match action {
        ServiceActionKind::Start => deps.services.start_all_non_core(),
        ServiceActionKind::Stop => deps.services.stop_all_non_core(force),
        ServiceActionKind::Restart => deps.services.restart_all_non_core(force),
    }
}

fn dispatch_single(
    action: ServiceActionKind,
    deps: &CliDependencies,
    id: &str,
    force: bool,
) -> Result<ServiceControlOutcome, ServiceControlError> {
    match action {
        ServiceActionKind::Start => deps.services.start_service(id),
        ServiceActionKind::Stop => deps.services.stop_service(id, force),
        ServiceActionKind::Restart => deps.services.restart_service(id, force),
    }
}

fn render_single_outcome(
    action: ServiceActionKind,
    id: &str,
    outcome: ServiceControlOutcome,
    out: &mut dyn Write,
) -> io::Result<()> {
    match (action, outcome) {
        (ServiceActionKind::Start, ServiceControlOutcome::Started) => {
            writeln!(out, "{}", control_messages::started(id))
        }
        (ServiceActionKind::Start, ServiceControlOutcome::AlreadyRunning) => {
            writeln!(out, "{}", control_messages::already_running(id))
        }
        (ServiceActionKind::Stop, ServiceControlOutcome::Stopped) => {
            writeln!(out, "{}", control_messages::stopped(id))
        }
        (ServiceActionKind::Stop, ServiceControlOutcome::AlreadyStopped) => {
            writeln!(out, "{}", control_messages::already_stopped(id))
        }
        (
            ServiceActionKind::Restart,
            ServiceControlOutcome::Restarted | ServiceControlOutcome::Started,
        ) => writeln!(out, "{}", control_messages::restarted(id)),
        (ServiceActionKind::Restart, ServiceControlOutcome::AlreadyRunning) => {
            writeln!(out, "{}", control_messages::already_running(id))
        }
        _ => {
            let unexpected = format!("{outcome:?}");
            writeln!(
                out,
                "{}",
                control_messages::unexpected_outcome(id, &unexpected)
            )
        }
    }
}

fn render_control_error(
    err: ServiceControlError,
    requested_id: &str,
    out: &mut dyn Write,
) -> io::Result<()> {
    match err {
        ServiceControlError::UnknownService(_) => {
            writeln!(out, "{}", control_messages::unknown_service(requested_id))?
        }
        ServiceControlError::NotControllable(_) => {
            writeln!(out, "{}", control_messages::not_controllable(requested_id))?
        }
        ServiceControlError::ForceRequired(_) => {
            writeln!(out, "{}", control_messages::force_required(requested_id))?
        }
        ServiceControlError::CoreLocked(_) => {
            writeln!(out, "{}", control_messages::core_locked(requested_id))?
        }
        ServiceControlError::OperationFailed { source, .. } => {
            let err = source.to_string();
            writeln!(out, "{}", control_messages::operation_failed(&err))?
        }
    }
    Ok(())
}

fn render_bulk_results(
    deps: &CliDependencies,
    out: &mut dyn Write,
    action: &str,
    force: bool,
    reports: Vec<ServiceActionReport>,
) -> io::Result<()> {
    if reports.is_empty() {
        writeln!(out, "{}", control_messages::NO_CONTROLLABLE_SERVICES)?;
        return Ok(());
    }
    writeln!(out, "{}", control_messages::bulk_header(action))?;
    let mut success = 0usize;
    let mut failures = Vec::new();
    for report in reports {
        match report.result {
            Ok(outcome) => {
                success += 1;
                writeln!(
                    out,
                    "{}",
                    control_messages::bulk_success_line(&report.id, outcome.as_str())
                )?
            }
            Err(err) => {
                let message = err.to_string();
                failures.push((report.id.clone(), message.clone()));
                writeln!(
                    out,
                    "{}",
                    control_messages::bulk_failure_line(&report.id, &message)
                )?;
            }
        }
    }
    record_cli_bulk_action(deps, action, force, success, failures.len(), failures);
    Ok(())
}

struct ParsedServiceArgs<'a> {
    first_value: Option<&'a str>,
    saw_all_flag: bool,
    saw_force_flag: bool,
}

impl<'a> ParsedServiceArgs<'a> {
    fn from(tokens: &'a [&'a str]) -> Self {
        let mut first_value = None;
        let mut saw_all_flag = false;
        let mut saw_force_flag = false;

        for token in tokens {
            match *token {
                "--force" | "-f" => {
                    saw_force_flag = true;
                }
                "--all" | "-a" | "all" => {
                    saw_all_flag = true;
                    if first_value.is_none() {
                        first_value = Some(*token);
                    }
                }
                _ => {
                    if first_value.is_none() {
                        first_value = Some(*token);
                    }
                }
            }
        }

        Self {
            first_value,
            saw_all_flag,
            saw_force_flag,
        }
    }

    fn force(&self) -> bool {
        self.saw_force_flag
    }

    fn apply_all(&self) -> bool {
        matches!(self.first_value, Some("--all" | "-a" | "all"))
            || (self.first_value.is_none() && self.saw_all_flag)
    }

    fn target(&self) -> Option<&'a str> {
        if self.apply_all() {
            None
        } else {
            self.first_value
        }
    }
}
