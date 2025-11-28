use std::io::{self, Write};

use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, CommandShape, ShellEnvironment,
};
use crate::services::db_shell::DESTRUCTIVE_FORCE_WARNING;
use crate::services::ServiceStatus;
use crate::utils::messages::cli::builtins::db_shell::command as db_shell_command_messages;

const DETAILS: &[&str] = &[
    db_shell_command_messages::DETAIL_SWITCH,
    db_shell_command_messages::DETAIL_TABLES,
    db_shell_command_messages::DETAIL_PING,
    db_shell_command_messages::DETAIL_EXIT,
    DESTRUCTIVE_FORCE_WARNING,
];

const DB_SHELL_SHAPE: CommandShape = CommandShape::basic("db-shell");

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        db_shell_command_messages::NAME,
        db_shell_command_messages::DESCRIPTION,
        db_shell_command_messages::USAGE,
        DETAILS,
        handle,
        DB_SHELL_SHAPE,
    )
}

fn handle(
    deps: &CliDependencies,
    _args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let service = &deps.services.db_shell;
    if !service.is_enabled() {
        writeln!(out, "{}", db_shell_command_messages::DISABLED_NOTE)?;
        return Ok(CommandOutcome::Continue);
    }
    let default_engine = service.default_engine();
    let engines = service
        .available_engines()
        .iter()
        .map(|engine| engine.to_string())
        .collect::<Vec<_>>();
    let engines_line = db_shell_command_messages::engines_line(&engines);
    let default_engine_label = default_engine.to_string();
    match env {
        ShellEnvironment::Cli => {
            deps.services.registry().set_status(
                "db-shell",
                ServiceStatus::Active,
                Some(db_shell_command_messages::STATUS_NOTE_CLI.to_string()),
            );
            writeln!(
                out,
                "{}",
                db_shell_command_messages::starting_cli(&default_engine_label, &engines_line)
            )?;
        }
        ShellEnvironment::Ssh => {
            deps.services.registry().set_status(
                "db-shell",
                ServiceStatus::Active,
                Some(db_shell_command_messages::STATUS_NOTE_SSH.to_string()),
            );
            writeln!(
                out,
                "{}",
                db_shell_command_messages::starting_ssh(&default_engine_label, &engines_line)
            )?;
        }
    }
    Ok(CommandOutcome::EnterDbShell)
}
