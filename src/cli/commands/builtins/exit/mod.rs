use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use std::io::{self, Write};

pub fn command() -> CommandEntry {
    CommandEntry::new("exit", "Beendet die aktuelle Sitzung", handle)
}

fn handle(
    _deps: &CliDependencies,
    _args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    writeln!(out, "Sitzung wird beendet ...")?;
    Ok(CommandOutcome::ExitShell)
}
