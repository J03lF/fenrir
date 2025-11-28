use std::io::{self, Write};

use rustyline::history::DefaultHistory;
use rustyline::{error::ReadlineError, Editor};
use tokio::runtime::{Handle, Runtime};
use tokio::task;

use crate::cli::completion::ListCompleter;
use crate::cli::shell::map_readline_error;
use crate::domain::db::DbResult;
use crate::services::db_shell::DbShellSession;
use crate::utils::messages::cli::builtins::db_shell::shell as db_shell_shell_messages;

use super::process::{process_command, sanitize_error, CommandResult};
use super::render::{render_execution_results, render_schema, render_tables};

pub fn run_local_db_shell(mut session: DbShellSession, prompt: String) -> io::Result<()> {
    let mut stdout = io::stdout();
    writeln!(
        &mut stdout,
        "{}",
        db_shell_shell_messages::active(&session.current_engine().to_string())
    )?;
    writeln!(&mut stdout, "{}", db_shell_shell_messages::HELP_HINT)?;

    let mut editor = Editor::<ListCompleter, DefaultHistory>::new().map_err(map_readline_error)?;
    editor.set_helper(Some(ListCompleter::new(completion_words(Some(&session)))));
    let executor = RuntimeExecutor::new()?;

    loop {
        match editor.readline(&prompt) {
            Ok(line) => {
                let command = line.trim();
                if command.is_empty() {
                    continue;
                }
                let _ = editor.add_history_entry(command);
                let continue_session =
                    apply_command(&mut session, command, &executor, &mut stdout)?;
                if !continue_session {
                    break;
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(ReadlineError::Io(err)) => return Err(err),
            Err(err) => {
                writeln!(
                    &mut stdout,
                    "{}{err}",
                    db_shell_shell_messages::INPUT_ERROR_PREFIX
                )?;
                break;
            }
        }
    }
    Ok(())
}

pub fn completion_words(session: Option<&DbShellSession>) -> Vec<String> {
    let mut words = vec![
        "help".to_string(),
        "exit".to_string(),
        r"\q".to_string(),
        r"\d".to_string(),
        r"\ping".to_string(),
        r"\c".to_string(),
    ];
    if let Some(session) = session {
        for engine in session.available_engines() {
            let value = engine.to_string();
            if !words.contains(&value) {
                words.push(value);
            }
        }
    }
    words.sort();
    words.dedup();
    words
}

pub fn apply_command(
    session: &mut DbShellSession,
    command: &str,
    executor: &RuntimeExecutor,
    out: &mut dyn Write,
) -> io::Result<bool> {
    match process_command(session, command, executor) {
        Ok(CommandResult::Exit) => {
            writeln!(out, "{}", db_shell_shell_messages::EXIT_MESSAGE)?;
            Ok(false)
        }
        Ok(CommandResult::Message(lines)) => {
            for line in lines {
                writeln!(out, "{line}")?;
            }
            Ok(true)
        }
        Ok(CommandResult::Execution(results)) => {
            render_execution_results(out, &results)?;
            Ok(true)
        }
        Ok(CommandResult::Tables(tables)) => {
            render_tables(out, &tables)?;
            Ok(true)
        }
        Ok(CommandResult::Schema(schema)) => {
            render_schema(out, &schema)?;
            Ok(true)
        }
        Err(err) => {
            writeln!(
                out,
                "{}{}",
                db_shell_shell_messages::ERROR_PREFIX,
                sanitize_error(&err)
            )?;
            Ok(true)
        }
    }
}

pub struct RuntimeExecutor {
    handle: Handle,
    #[allow(dead_code)]
    runtime: Option<Runtime>,
}

impl RuntimeExecutor {
    pub fn new() -> io::Result<Self> {
        if let Ok(handle) = Handle::try_current() {
            Ok(Self {
                handle,
                runtime: None,
            })
        } else {
            let runtime = Runtime::new().map_err(|err| io::Error::other(err.to_string()))?;
            let handle = runtime.handle().clone();
            Ok(Self {
                handle,
                runtime: Some(runtime),
            })
        }
    }

    pub fn run<T>(&self, future: impl std::future::Future<Output = DbResult<T>>) -> DbResult<T> {
        if Handle::try_current().is_ok() {
            task::block_in_place(|| self.handle.block_on(future))
        } else {
            self.handle.block_on(future)
        }
    }
}
