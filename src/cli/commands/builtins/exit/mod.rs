use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, CommandShape, ShellEnvironment,
};
use std::io::{self, Write};

const DETAILS: &[&str] = &[
    "exit – beendet die aktuelle CLI/SSH-Sitzung",
    "In Subshells (z. B. db-shell) kehrt 'exit' zur Hauptshell zurück",
];

const EXIT_SHAPE: CommandShape = CommandShape::basic("exit");

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "exit",
        "Beendet die aktuelle Sitzung",
        "exit",
        DETAILS,
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
    writeln!(out, "Sitzung wird beendet ...")?;
    Ok(CommandOutcome::ExitShell)
}
