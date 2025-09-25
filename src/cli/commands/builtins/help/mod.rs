use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use std::io::{self, Write};

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "help",
        "Listet Befehle oder zeigt Details zu einem Befehl an",
        handle,
    )
}

fn handle(
    _deps: &CliDependencies,
    args: &[&str],
    registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    if let Some(command_name) = args.first() {
        match registry.get(command_name) {
            Some(entry) => {
                writeln!(out, "{} - {}", entry.name, entry.description)?;
            }
            None => {
                writeln!(out, "unbekannter Befehl: {command_name}")?;
            }
        }
    } else {
        writeln!(out, "Verfügbare Befehle:")?;
        for entry in registry.entries() {
            writeln!(out, "  {:<12} {}", entry.name, entry.description)?;
        }
        writeln!(out, "Nutze 'help <befehl>' für Details")?;
    }
    Ok(CommandOutcome::Continue)
}
