use super::control::{restart_service, start_service, stop_service};
use super::list::{list_jobs, list_services};
use super::metadata::{metadata_for_action, ListResource, ServiceActionResource};
use crate::cli::commands::builtins::jobs::{pause_job_cli, restart_job_cli, resume_job_cli};
use crate::cli::commands::builtins::modules;
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::services::ServiceActionKind;
use crate::utils::messages::cli::builtins::jobs as job_messages;
use crate::utils::messages::cli::builtins::services::actions as msg_actions;
use std::io::{self, Write};

pub(super) fn handle_start(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    route_service_action(ServiceActionKind::Start, deps, args, out)
}

pub(super) fn handle_stop(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    route_service_action(ServiceActionKind::Stop, deps, args, out)
}

pub(super) fn handle_restart(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    route_service_action(ServiceActionKind::Restart, deps, args, out)
}

pub(super) fn handle_pause(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    dispatch_job_action(deps, args, job_messages::pause::USAGE, out, pause_job_cli)
}

pub(super) fn handle_resume(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    dispatch_job_action(deps, args, job_messages::resume::USAGE, out, resume_job_cli)
}

pub(super) fn handle_list(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let (resource, rest) = match args.split_first() {
        Some((value, tail)) => match ListResource::parse(value) {
            Some(resource) => (resource, tail),
            None => {
                writeln!(out, "{}", msg_actions::unknown_list_resource(value))?;
                writeln!(out, "{}", msg_actions::available_resources_hint())?;
                return Ok(CommandOutcome::Continue);
            }
        },
        None => (ListResource::default(), &[][..]),
    };

    if !rest.is_empty() {
        writeln!(
            out,
            "{}",
            msg_actions::list_argument_hint(resource.primary_name())
        )?;
    }

    match resource {
        ListResource::Services => {
            list_services(deps, out)?;
        }
        ListResource::Jobs => {
            list_jobs(deps, out)?;
        }
        ListResource::Modules => {
            return modules::run_module_command(deps, "list", rest, out);
        }
    }
    Ok(CommandOutcome::Continue)
}

fn dispatch_job_action<F>(
    deps: &CliDependencies,
    args: &[&str],
    usage: &str,
    out: &mut dyn Write,
    executor: F,
) -> io::Result<CommandOutcome>
where
    F: Fn(&CliDependencies, &str, &mut dyn Write) -> io::Result<()>,
{
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "{usage}")?;
        return Ok(CommandOutcome::Continue);
    };
    let Some(action_resource) = ServiceActionResource::parse(resource) else {
        writeln!(out, "{}", msg_actions::unknown_action_resource(resource))?;
        writeln!(out, "{usage}")?;
        return Ok(CommandOutcome::Continue);
    };
    if action_resource != ServiceActionResource::Job {
        writeln!(out, "{usage}")?;
        return Ok(CommandOutcome::Continue);
    }
    let Some(job_id) = tail.first() else {
        writeln!(out, "{usage}")?;
        return Ok(CommandOutcome::Continue);
    };
    executor(deps, job_id, out)?;
    Ok(CommandOutcome::Continue)
}

fn route_service_action(
    action: ServiceActionKind,
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    if args.is_empty() {
        writeln!(out, "{}", metadata_for_action(action).usage())?;
        return Ok(CommandOutcome::Continue);
    }

    let Some(resource) = ServiceActionResource::parse(args[0]) else {
        writeln!(out, "{}", msg_actions::unknown_action_resource(args[0]))?;
        writeln!(out, "{}", msg_actions::ACTION_RESOURCE_HINT)?;
        return Ok(CommandOutcome::Continue);
    };
    let tail = if args.len() > 1 { &args[1..] } else { &[] };

    match resource {
        ServiceActionResource::Service => {
            match action {
                ServiceActionKind::Start => start_service(deps, tail, out),
                ServiceActionKind::Stop => stop_service(deps, tail, out),
                ServiceActionKind::Restart => restart_service(deps, tail, out),
            }?;
            Ok(CommandOutcome::Continue)
        }
        ServiceActionResource::Module => {
            modules::run_module_command(deps, action.verb(), tail, out)
        }
        ServiceActionResource::Job => {
            if action != ServiceActionKind::Restart {
                writeln!(
                    out,
                    "{}",
                    msg_actions::job_action_unsupported(action.verb())
                )?;
                return Ok(CommandOutcome::Continue);
            }
            let Some(job_id) = tail.first() else {
                writeln!(out, "{}", msg_actions::missing_job_id())?;
                return Ok(CommandOutcome::Continue);
            };
            restart_job_cli(deps, job_id, out)?;
            Ok(CommandOutcome::Continue)
        }
    }
}
