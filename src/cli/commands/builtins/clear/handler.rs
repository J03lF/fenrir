use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::prompts;
use std::io::{self, Write};

pub(super) fn handle(
    _deps: &CliDependencies,
    _args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    write!(out, "{}", prompts::clear_screen_sequence())?;
    out.flush()?;
    Ok(CommandOutcome::Continue)
}
