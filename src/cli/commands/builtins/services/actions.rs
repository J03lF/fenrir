use super::control::{restart_service, start_service, stop_service};
use super::list::{list_jobs, list_services};
use super::metadata::{metadata_for_action, ListResource, ServiceActionResource};
use crate::cli::commands::builtins::modules;
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::services::ServiceActionKind;
use crate::utils::messages::cli::builtins::services::{actions as msg_actions, module_guard};
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

    match resource {
        ServiceActionResource::Service => {
            let tail = if args.len() > 1 { &args[1..] } else { &[] };
            match action {
                ServiceActionKind::Start => start_service(deps, tail, out),
                ServiceActionKind::Stop => stop_service(deps, tail, out),
                ServiceActionKind::Restart => restart_service(deps, tail, out),
            }?;
            Ok(CommandOutcome::Continue)
        }
        ServiceActionResource::Module => {
            writeln!(out, "{}", module_guard::action_disabled_line(action.verb()))?;
            writeln!(out, "{}", module_guard::AUTO_MANAGED_SUMMARY)?;
            Ok(CommandOutcome::Continue)
        }
    }
}
