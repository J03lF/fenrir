use std::io::{self, Write};

use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, CommandShape, ShellEnvironment,
};
use crate::utils::messages::cli::builtins::exit::{
    command as exit_messages, handler as exit_handler_messages,
};

const EXIT_SHAPE: CommandShape = CommandShape::basic("exit");

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        exit_messages::NAME,
        exit_messages::DESCRIPTION,
        exit_messages::USAGE,
        exit_messages::DETAILS,
        handle,
        EXIT_SHAPE,
    )
}

fn handle(
    _deps: &CliDependencies,
    _args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    writeln!(out, "{}", exit_handler_messages::shutting_down())?;
    Ok(CommandOutcome::ExitShell)
}
