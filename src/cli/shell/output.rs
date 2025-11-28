use crate::cli::commands::registry::CommandOutcome;
use crate::utils;
use crate::utils::messages::cli::shell::output as shell_output_messages;
use std::io::{self, Write};
use std::time::Duration;

const COLOR_RESET: &str = "\x1b[0m";
const COLOR_DIM: &str = "\x1b[38;5;244m";
const COLOR_SUCCESS: &str = "\x1b[38;5;76m";
const COLOR_ERROR: &str = "\x1b[38;5;203m";

pub(super) fn show_pending(out: &mut dyn Write, command: &str, args: &[&str]) -> io::Result<()> {
    let joined = if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.join(" "))
    };
    writeln!(
        out,
        "{dim}→{reset} {text}",
        dim = COLOR_DIM,
        reset = COLOR_RESET,
        text = joined
    )?;
    out.flush()
}

pub(super) fn show_success(
    out: &mut dyn Write,
    command: &str,
    args: &[&str],
    duration: Duration,
    outcome: &CommandOutcome,
) -> io::Result<()> {
    let joined = if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.join(" "))
    };
    let (symbol, note) = match outcome {
        CommandOutcome::Continue => ("✔", shell_output_messages::STATUS_OK),
        CommandOutcome::ExitShell => ("✔", shell_output_messages::STATUS_EXIT),
        CommandOutcome::EnterDbShell => ("✔", shell_output_messages::STATUS_DB),
        CommandOutcome::AwaitConfirmation(_) => ("?", shell_output_messages::STATUS_CONFIRM),
        CommandOutcome::AsyncTask(_) => ("⇄", shell_output_messages::STATUS_ASYNC),
    };
    writeln!(
        out,
        "{color}{symbol}{reset} {cmd} [{note} • {time}]",
        color = COLOR_SUCCESS,
        reset = COLOR_RESET,
        cmd = joined,
        note = note,
        symbol = symbol,
        time = utils::format_brief_duration(duration)
    )?;
    out.flush()
}

pub(super) fn show_error(
    out: &mut dyn Write,
    code: &str,
    message: String,
    duration: Duration,
) -> io::Result<()> {
    let hint = if duration.is_zero() {
        String::new()
    } else {
        format!(" • {}", utils::format_brief_duration(duration))
    };
    writeln!(
        out,
        "{color}✖ {code}{reset} {msg}{hint}",
        color = COLOR_ERROR,
        code = code,
        reset = COLOR_RESET,
        msg = message,
        hint = hint
    )?;
    out.flush()
}
