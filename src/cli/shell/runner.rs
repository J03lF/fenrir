use crate::cli::commands::builtins;
use crate::cli::commands::registry::{
    parse_confirmation_answer, CliDependencies, CommandOutput, CommandStatus, ConfirmationRequest,
    ShellEnvironment,
};
use crate::cli::completion::ContextualCompleter;
use crate::config::AppConfig;
use crate::prompts::{self, PromptContext};
use crate::services::{AppServices, ServiceStatus};
use crate::utils::messages::cli::shell::runner as shell_runner_messages;
use rustyline::history::DefaultHistory;
use rustyline::{error::ReadlineError, Editor};
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::history::init_history;
use super::map_readline_error;
use super::outcome::handle_outcome;
use super::output::{show_error, show_pending, show_success};

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
    write!(&mut stdout, "{}", prompts::clear_screen_sequence())?;
    writeln!(&mut stdout, "{}", prompts::banner())?;
    writeln!(&mut stdout, "{}", prompts::welcome_line(config.as_ref()))?;

    let registry = builtins::build_registry();
    let dependencies = CliDependencies::new(Arc::clone(&config), Arc::clone(&services))
        .with_output(Arc::new(StdoutCommandOutput));

    services.registry().set_status(
        "cli-shell",
        ServiceStatus::Active,
        Some(shell_runner_messages::local_session_note(std::process::id())),
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
                                    shell_runner_messages::confirmation_failed(&err),
                                    started.elapsed(),
                                )?;
                            }
                        }
                    } else {
                        writeln!(&mut stdout, "{}", shell_runner_messages::CONFIRMATION_RETRY)?;
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
                                shell_runner_messages::confirmation_failed(&err),
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
                        shell_runner_messages::input_error(&err),
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
                                shell_runner_messages::command_unknown(name),
                                duration,
                            )?;
                        }
                        Err(err) => {
                            let duration = started.elapsed();
                            show_error(
                                &mut stdout,
                                "CLI-0002",
                                shell_runner_messages::command_aborted(name, &err),
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
                    shell_runner_messages::input_error(&err),
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
        Some(shell_runner_messages::CLI_STANDBY_NOTE.to_string()),
    );
    Ok(())
}
