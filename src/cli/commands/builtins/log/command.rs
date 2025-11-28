use std::io::{self, Write};

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CommandSubcommand, CompletionKind, ShellEnvironment,
};
use crate::infra::logging::{self, LogKind};
use crate::utils::messages::cli::builtins::log::{
    command as log_command_messages, handler as log_handler_messages,
};
use tracing::{info, warn};

use super::launch::{display_log_result, parse_target};

const LOG_LEVEL_OPTIONS: &[&str] = &["trace", "debug", "info", "warn", "error"];
const LOG_ARCHIVE_OPTIONS: &[&str] = &["app", "db"];

const LOG_LEVEL_ARGUMENT: CommandArgument =
    CommandArgument::required("level").with_completion(CompletionKind::Static(LOG_LEVEL_OPTIONS));
const LOG_ARCHIVE_ARGUMENT: CommandArgument = CommandArgument::optional("target")
    .with_completion(CompletionKind::Static(LOG_ARCHIVE_OPTIONS));

const LOG_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new("app", &[], &[], log_command_messages::SUB_APP_DESCRIPTION),
    CommandSubcommand::new("db", &[], &[], log_command_messages::SUB_DB_DESCRIPTION),
    CommandSubcommand::new("all", &[], &[], log_command_messages::SUB_ALL_DESCRIPTION),
    CommandSubcommand::new(
        "level",
        &[],
        &[LOG_LEVEL_ARGUMENT],
        log_command_messages::SUB_LEVEL_DESCRIPTION,
    ),
    CommandSubcommand::new(
        "archive",
        &[],
        &[LOG_ARCHIVE_ARGUMENT],
        log_command_messages::SUB_ARCHIVE_DESCRIPTION,
    ),
];

const LOG_SHAPE: CommandShape = CommandShape::new("log", &[], &[], LOG_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        log_command_messages::NAME,
        log_command_messages::DESCRIPTION,
        log_command_messages::USAGE,
        log_command_messages::DETAILS,
        handle,
        LOG_SHAPE,
    )
}

fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    if args.is_empty() {
        info!(command = "log", target = "app", "log command invoked");
        display_log_result(
            out,
            log_handler_messages::DEFAULT_LABEL_APP,
            logging::log_file_path(),
        )?;
        return Ok(CommandOutcome::Continue);
    }

    let mut iter = args.iter().copied();
    let action = iter.next().unwrap_or("app");
    match action {
        "app" | "db" => {
            let kind = parse_target(action).unwrap();
            info!(command = "log", target = action, "log command invoked");
            let path = match kind {
                LogKind::App => logging::log_file_path(),
                LogKind::Db => logging::db_log_file_path(),
            };
            let label = match kind {
                LogKind::App => log_handler_messages::DEFAULT_LABEL_APP,
                LogKind::Db => log_handler_messages::DEFAULT_LABEL_DB,
            };
            display_log_result(out, label, path)?;
        }
        "all" => {
            info!(command = "log", target = "all", "log command invoked");
            display_log_result(
                out,
                log_handler_messages::DEFAULT_LABEL_APP,
                logging::log_file_path(),
            )?;
            display_log_result(
                out,
                log_handler_messages::DEFAULT_LABEL_DB,
                logging::db_log_file_path(),
            )?;
        }
        "level" => {
            let Some(level) = iter.next() else {
                writeln!(out, "{}", log_handler_messages::MISSING_LEVEL_USAGE)?;
                return Ok(CommandOutcome::Continue);
            };
            if let Some(handle) = deps.services.logging_handle() {
                match logging::reload(&handle, level) {
                    Ok(()) => {
                        writeln!(out, "{}", log_handler_messages::level_updated(level))?;
                        info!(
                            command = "log",
                            mode = "level",
                            value = level,
                            "log level updated"
                        );
                    }
                    Err(err) => {
                        writeln!(out, "{}", log_handler_messages::level_reload_failed(&err))?;
                        tracing::warn!(error = %err, "failed to reload log level");
                    }
                }
            } else {
                writeln!(out, "{}", log_handler_messages::NO_RELOAD_HANDLE)?;
            }
        }
        "archive" => {
            let target = iter.next().unwrap_or("app");
            if let Some(kind) = parse_target(target) {
                info!(
                    command = "log",
                    target = target,
                    mode = "archive",
                    "log command invoked"
                );
                let label = match kind {
                    LogKind::App => log_handler_messages::ARCHIVE_LABEL_APP,
                    LogKind::Db => log_handler_messages::ARCHIVE_LABEL_DB,
                };
                let archive = logging::latest_archive(kind);
                display_log_result(out, label, archive)?;
            } else {
                writeln!(
                    out,
                    "{}",
                    log_handler_messages::unknown_archive_target(target)
                )?;
            }
        }
        other => {
            warn!(command = "log", target = other, "unknown log subcommand");
            writeln!(out, "{}", log_handler_messages::unknown_action(other))?;
        }
    }

    Ok(CommandOutcome::Continue)
}
