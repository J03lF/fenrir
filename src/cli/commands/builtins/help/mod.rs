use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use std::io::{self, Write};

const HELP_DETAILS: &[&str] = &[
    "help            – listet alle Befehle",
    "help <befehl>   – zeigt Details, Usage und Optionen",
];

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "help",
        "Listet Befehle oder zeigt Details zu einem Befehl an",
        "help [befehl]",
        HELP_DETAILS,
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
                writeln!(out, "Usage: {}", entry.usage)?;
                if !entry.details.is_empty() {
                    writeln!(out, "Details:")?;
                    for line in entry.details {
                        writeln!(out, "  - {}", line)?;
                    }
                }
            }
            None => {
                writeln!(out, "unbekannter Befehl: {command_name}")?;
            }
        }
    } else {
        writeln!(out, "Verfügbare Befehle:")?;
        let mut table = Table::new(vec![
            "Befehl".to_string(),
            "Usage".to_string(),
            "Beschreibung".to_string(),
        ]);
        for entry in registry.entries() {
            table.add_row(vec![
                entry.name.clone(),
                entry.usage.to_string(),
                entry.description.clone(),
            ]);
        }
        table.render(out, "  ")?;
        writeln!(out, "Nutze 'help <befehl>' für Details")?;
    }
    Ok(CommandOutcome::Continue)
}
