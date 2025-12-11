use crate::cli::commands::builtins::modules;
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::utils::messages::cli::builtins::show::handler as show_handler_messages;
use std::io::{self, Write};

pub(super) fn handle_show(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "{}", show_handler_messages::USAGE_GENERIC)?;
        return Ok(CommandOutcome::Continue);
    };

    if resource.eq_ignore_ascii_case("module") {
        if tail.is_empty() {
            writeln!(out, "{}", show_handler_messages::USAGE_MODULE)?;
        } else {
            return modules::run_module_command(deps, "info", tail, out);
        }
    } else {
        writeln!(out, "{}", show_handler_messages::unknown_resource(resource))?;
        writeln!(out, "{}", show_handler_messages::VALID_RESOURCES)?;
    }

    Ok(CommandOutcome::Continue)
}
