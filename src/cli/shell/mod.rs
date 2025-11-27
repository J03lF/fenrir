use crate::cli::commands::builtins;
use crate::cli::commands::registry::{
    parse_confirmation_answer, CliDependencies, CommandOutcome, CommandStatus, ConfirmationRequest,
    ShellEnvironment, CommandOutput,
};
use crate::cli::completion::ContextualCompleter;
use crate::config::AppConfig;
use crate::prompts::{self, PromptContext};
use crate::services::{AppServices, ServiceStatus};
use crate::utils;
use futures::executor::block_on;
use rustyline::history::{DefaultHistory, History};
use rustyline::{error::ReadlineError, Editor};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const COLOR_RESET: &str = "\x1b[0m";
const COLOR_DIM: &str = "\x1b[38;5;244m";
const COLOR_SUCCESS: &str = "\x1b[38;5;76m";
const COLOR_ERROR: &str = "\x1b[38;5;203m";

pub fn run_shell(config: Arc<AppConfig>, services: Arc<AppServices>) -> io::Result<()> {
    #[derive(Clone)]
    struct StdoutCommandOutput;
    impl CommandOutput for StdoutCommandOutput {
        fn push(&self, text: &str) {
            let mut out = io::stdout();
            let _ = out.write_all(text.as_bytes());
            let _ = out.flush();
        }
    }

    let mut stdout = io::stdout();
    // Clear screen and position cursor in the top left before showing the banner.
    write!(&mut stdout, "{}", prompts::clear_screen_sequence())?;
    writeln!(&mut stdout, "{}", prompts::banner())?;
    writeln!(&mut stdout, "{}", prompts::welcome_line(config.as_ref()))?;

    let registry = builtins::build_registry();
    let dependencies = CliDependencies::new(Arc::clone(&config), Arc::clone(&services))
        .with_output(Arc::new(StdoutCommandOutput));

    services.registry().set_status(
        "cli-shell",
        ServiceStatus::Active,
        Some(format!("lokale Sitzung pid={}", std::process::id())),
    );

    let prompt_context = PromptContext::local_default(&config.server.ssh.server_name);
    let prompt_set = prompts::prompt_set(config.as_ref(), &prompt_context);

    let mut editor =
        Editor::<ContextualCompleter, DefaultHistory>::new().map_err(map_readline_error)?;
    let shapes = registry.shapes();
    editor.set_helper(Some(ContextualCompleter::new(
        shapes.clone(),
        dependencies.clone(),
        ShellEnvironment::Cli,
    )));
    tracing::debug!(
        target = "cli::shell",
        commands = shapes.len(),
        "rustyline helper registered"
    );
    if let Some(helper) = editor.helper_mut() {
        helper.update_catalog(registry.shapes());
    }
    let history_path = init_history(&mut editor);
    let mut pending_confirmation: Option<ConfirmationRequest> = None;
    loop {
        if let Some(request) = pending_confirmation.take() {
            match editor.readline(request.prompt()) {
                Ok(line) => {
                    if let Some(answer) = parse_confirmation_answer(&line) {
                        let context = request.command_context();
                        let started = Instant::now();
                        match request.resolve(answer, &dependencies, &mut stdout) {
                            Ok(outcome) => {
                                let duration = started.elapsed();
                                if let Some((cmd, cmd_args)) = context.as_ref() {
                                    let arg_refs: Vec<&str> =
                                        cmd_args.iter().map(|s| s.as_str()).collect();
                                    show_success(
                                        &mut stdout,
                                        cmd.as_str(),
                                        &arg_refs,
                                        duration,
                                        &outcome,
                                    )?;
                                }
                                if !handle_outcome(
                                    &mut stdout,
                                    &services,
                                    &prompt_set,
                                    &mut pending_confirmation,
                                    outcome,
                                )? {
                                    break;
                                }
                            }
                            Err(err) => {
                                show_error(
                                    &mut stdout,
                                    "CLI-0004",
                                    format!("Bestätigung fehlgeschlagen: {err}"),
                                    started.elapsed(),
                                )?;
                            }
                        }
                    } else {
                        writeln!(&mut stdout, "Bitte mit 'y' oder 'n' bestätigen.")?;
                        pending_confirmation = Some(request);
                    }
                }
                Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => {
                    let context = request.command_context();
                    let started = Instant::now();
                    match request.resolve(false, &dependencies, &mut stdout) {
                        Ok(outcome) => {
                            let duration = started.elapsed();
                            if let Some((cmd, cmd_args)) = context.as_ref() {
                                let arg_refs: Vec<&str> =
                                    cmd_args.iter().map(|s| s.as_str()).collect();
                                show_success(
                                    &mut stdout,
                                    cmd.as_str(),
                                    &arg_refs,
                                    duration,
                                    &outcome,
                                )?;
                            }
                            if !handle_outcome(
                                &mut stdout,
                                &services,
                                &prompt_set,
                                &mut pending_confirmation,
                                outcome,
                            )? {
                                break;
                            }
                        }
                        Err(err) => {
                            show_error(
                                &mut stdout,
                                "CLI-0004",
                                format!("Bestätigung fehlgeschlagen: {err}"),
                                started.elapsed(),
                            )?;
                        }
                    }
                }
                Err(ReadlineError::Io(err)) => return Err(err),
                Err(err) => {
                    show_error(
                        &mut stdout,
                        "CLI-0003",
                        format!("Eingabefehler: {err}"),
                        Duration::from_secs(0),
                    )?;
                    break;
                }
            }
            continue;
        }
        match editor.readline(&prompt_set.main_cli) {
            Ok(line) => {
                let cmd = line.trim();
                if cmd.is_empty() {
                    continue;
                }

                let _ = editor.add_history_entry(cmd);

                let mut parts = cmd.split_whitespace();
                if let Some(name) = parts.next() {
                    let args: Vec<&str> = parts.collect();
                    show_pending(&mut stdout, name, &args)?;
                    let started = Instant::now();
                    match registry.execute(
                        name,
                        &args,
                        &dependencies,
                        &mut stdout,
                        ShellEnvironment::Cli,
                    ) {
                        Ok(CommandStatus::Executed(outcome)) => {
                            let duration = started.elapsed();
                            show_success(&mut stdout, name, &args, duration, &outcome)?;
                            if !handle_outcome(
                                &mut stdout,
                                &services,
                                &prompt_set,
                                &mut pending_confirmation,
                                outcome,
                            )? {
                                break;
                            }
                        }
                        Ok(CommandStatus::NotFound) => {
                            let duration = started.elapsed();
                            show_error(
                                &mut stdout,
                                "CLI-0001",
                                format!(
                                    "Befehl '{name}' unbekannt – nutze 'help' oder Tab für Vorschläge"
                                ),
                                duration,
                            )?;
                        }
                        Err(err) => {
                            let duration = started.elapsed();
                            show_error(
                                &mut stdout,
                                "CLI-0002",
                                format!("Befehl '{name}' abgebrochen: {err}"),
                                duration,
                            )?;
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(ReadlineError::Io(err)) => return Err(err),
            Err(err) => {
                show_error(
                    &mut stdout,
                    "CLI-0003",
                    format!("Eingabefehler: {err}"),
                    Duration::from_secs(0),
                )?;
                break;
            }
        }
    }
    if let Some(path) = history_path {
        let _ = editor.save_history(&path);
    }
    services.registry().set_status(
        "cli-shell",
        ServiceStatus::Standby,
        Some("Wartet auf nächste Sitzung".to_string()),
    );
    Ok(())
}

fn init_history<H, I>(editor: &mut Editor<H, I>) -> Option<PathBuf>
where
    H: rustyline::Helper,
    I: History,
{
    history_path().map(|path| {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = editor.load_history(&path);
        path
    })
}

fn history_path() -> Option<PathBuf> {
    let mut path = std::env::var_os("FENRIR_HISTORY_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))?;
    path.push("fenrir");
    path.push("cli");
    path.push("history.txt");
    Some(path)
}

fn map_readline_error(err: ReadlineError) -> io::Error {
    match err {
        ReadlineError::Io(err) => err,
        other => io::Error::new(io::ErrorKind::Other, other.to_string()),
    }
}

fn handle_outcome(
    out: &mut dyn Write,
    services: &AppServices,
    prompt_set: &prompts::PromptSet,
    pending_confirmation: &mut Option<ConfirmationRequest>,
    outcome: CommandOutcome,
) -> io::Result<bool> {
    match outcome {
        CommandOutcome::Continue => Ok(true),
        CommandOutcome::ExitShell => Ok(false),
        CommandOutcome::EnterDbShell => {
            let session = services.db_shell.create_session();
            if let Err(err) = crate::cli::commands::builtins::db_shell::run_local_db_shell(
                session,
                prompt_set.db_cli.clone(),
            ) {
                writeln!(out, "db-shell Fehler: {err}")?;
                services.registry().set_status(
                    "db-shell",
                    ServiceStatus::Degraded,
                    Some(format!("Fehler: {err}")),
                );
            } else {
                services.registry().set_status(
                    "db-shell",
                    ServiceStatus::Active,
                    Some("Bereit für neue Sessions".to_string()),
                );
            }
            Ok(true)
        }
        CommandOutcome::AwaitConfirmation(request) => {
            out.flush()?;
            *pending_confirmation = Some(request);
            Ok(true)
        }
        CommandOutcome::AsyncTask(task) => {
            let result = block_on(task)?;
            handle_outcome(out, services, prompt_set, pending_confirmation, result)
        }
    }
}

fn show_pending(out: &mut dyn Write, command: &str, args: &[&str]) -> io::Result<()> {
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

fn show_success(
    out: &mut dyn Write,
    command: &str,
    args: &[&str],
    duration: std::time::Duration,
    outcome: &CommandOutcome,
) -> io::Result<()> {
    let joined = if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.join(" "))
    };
    let (symbol, note) = match outcome {
        CommandOutcome::Continue => ("✔", "ok"),
        CommandOutcome::ExitShell => ("✔", "exit"),
        CommandOutcome::EnterDbShell => ("✔", "db"),
        CommandOutcome::AwaitConfirmation(_) => ("?", "confirm"),
        CommandOutcome::AsyncTask(_) => ("⇄", "async"),
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

fn show_error(
    out: &mut dyn Write,
    code: &str,
    message: String,
    duration: std::time::Duration,
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
