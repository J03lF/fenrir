use std::io::{self, Write};

use crate::cli::commands::builtins::jobs::{parse_tail_flag, stream_job_logs};
use crate::cli::commands::builtins::modules::{self, complete_module_ids};
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
const LOG_MODULE_ARGUMENT: CommandArgument = CommandArgument::required("module")
    .with_completion(CompletionKind::Dynamic(complete_module_ids));
const LOG_MODULE_TAIL_ARGUMENT: CommandArgument = CommandArgument {
    name: "--tail",
    optional: true,
    variadic: false,
    completion: CompletionKind::Static(&["--tail"]),
};
const LOG_JOB_ARGUMENT: CommandArgument = CommandArgument::required("job_id").with_completion(
    CompletionKind::Dynamic(crate::cli::commands::builtins::jobs::complete_job_ids),
);
const LOG_JOB_TAIL_ARGUMENT: CommandArgument = LOG_MODULE_TAIL_ARGUMENT;

const LOG_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new("app", &[], &[], log_command_messages::SUB_APP_DESCRIPTION),
    CommandSubcommand::new(
        "db",
        &["database"],
        &[LOG_MODULE_TAIL_ARGUMENT],
        "Tail embedded db-runtime logs",
    ),
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
    CommandSubcommand::new(
        "module",
        &[],
        &[LOG_MODULE_ARGUMENT, LOG_MODULE_TAIL_ARGUMENT],
        log_command_messages::SUB_MODULE_DESCRIPTION,
    ),
    CommandSubcommand::new(
        "env",
        &[],
        &[LOG_MODULE_ARGUMENT],
        log_command_messages::SUB_ENV_DESCRIPTION,
    ),
    CommandSubcommand::new(
        "job",
        &[],
        &[LOG_JOB_ARGUMENT, LOG_JOB_TAIL_ARGUMENT],
        log_command_messages::SUB_JOB_DESCRIPTION,
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
        "app" => {
            info!(command = "log", target = action, "log command invoked");
            display_log_result(
                out,
                log_handler_messages::DEFAULT_LABEL_APP,
                logging::log_file_path(),
            )?;
        }
        "db" | "database" | "db-runtime" => {
            // Verb-first: "log db" shows embedded db-runtime logs
            let tail_args: Vec<_> = iter.collect();
            let tail = match parse_tail_flag(&tail_args) {
                Ok(value) => value,
                Err(msg) => {
                    writeln!(out, "{msg}")?;
                    return Ok(CommandOutcome::Continue);
                }
            };
            let logs = deps.services.db_runtime_logs(tail.max(1));
            if logs.is_empty() {
                writeln!(out, "No db-runtime logs available (mode != embedded?)")?;
            } else {
                for line in logs {
                    writeln!(out, "{line}")?;
                }
            }
        }
        "db-log" => {
            // Access the db.log file (old behavior)
            info!(command = "log", target = "db-log", "log command invoked");
            display_log_result(
                out,
                log_handler_messages::DEFAULT_LABEL_DB,
                logging::db_log_file_path(),
            )?;
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
        "module" => {
            let tail_args: Vec<_> = iter.collect();
            let tail = strip_module_keyword(&tail_args);
            if tail.is_empty() {
                writeln!(out, "{}", log_handler_messages::MODULE_LOG_USAGE)?;
                return Ok(CommandOutcome::Continue);
            }
            return modules::run_module_command(deps, "log", tail, out);
        }
        "env" => {
            let tail_args: Vec<_> = iter.collect();
            let tail = strip_module_keyword(&tail_args);
            if tail.is_empty() {
                writeln!(out, "{}", log_handler_messages::MODULE_ENV_USAGE)?;
                return Ok(CommandOutcome::Continue);
            }
            return modules::run_module_command(deps, "env", tail, out);
        }
        "job" => {
            let tail_args: Vec<_> = iter.collect();
            if tail_args.is_empty() {
                writeln!(out, "{}", log_handler_messages::JOB_LOG_USAGE)?;
                return Ok(CommandOutcome::Continue);
            }
            let job_id = tail_args[0];
            let tail_tokens = &tail_args[1..];
            let tail = match parse_tail_flag(tail_tokens) {
                Ok(value) => value,
                Err(msg) => {
                    writeln!(out, "{msg}")?;
                    return Ok(CommandOutcome::Continue);
                }
            };
            stream_job_logs(deps, job_id, tail, out)?;
        }
        other => {
            warn!(command = "log", target = other, "unknown log subcommand");
            writeln!(out, "{}", log_handler_messages::unknown_action(other))?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn strip_module_keyword<'a>(args: &'a [&'a str]) -> &'a [&'a str] {
    if let Some(first) = args.first() {
        if first.eq_ignore_ascii_case("module") {
            return &args[1..];
        }
    }
    args
}
