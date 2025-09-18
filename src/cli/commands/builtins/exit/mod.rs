use crate::cli::commands::registry::{
    CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::config::AppConfig;
use std::io::{self, Write};

pub fn command() -> CommandEntry {
    CommandEntry::new("exit", "Beendet die aktuelle Sitzung", handle)
}

fn handle(
    _config: &AppConfig,
    _args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    writeln!(out, "Sitzung wird beendet ...")?;
    Ok(CommandOutcome::ExitShell)
}
