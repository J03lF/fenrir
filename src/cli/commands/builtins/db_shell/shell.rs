use std::io::{self, Write};

use rustyline::history::DefaultHistory;
use rustyline::{error::ReadlineError, CompletionType, Config, Editor};
use tokio::runtime::{Handle, Runtime};
use tokio::task;
use tracing::debug;

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
    writeln!(&mut stdout, "{}", db_shell_shell_messages::MULTILINE_HINT)?;

    // Configure editor: suppress rustyline's vertical list display
    // Our ListCompleter prints a horizontal grid instead
    let config = Config::builder()
        .completion_type(CompletionType::Circular)
        .completion_prompt_limit(0) // Don't show rustyline's vertical list
        .build();
    let mut editor = Editor::<ListCompleter, DefaultHistory>::with_config(config)
        .map_err(map_readline_error)?;
    let executor = RuntimeExecutor::new()?;
    
    // Build completion list with table names
    let (completion_list, table_count) = build_completion_list_with_tables(&session, &executor, &mut stdout);
    editor.set_helper(Some(ListCompleter::new(completion_list)));
    
    if table_count > 0 {
        writeln!(&mut stdout, "completion: {} tables loaded", table_count)?;
    } else {
        writeln!(&mut stdout, "completion: no tables loaded (type \\d to see tables)")?;
    }

    // Multi-line buffer for SQL statements
    let mut buffer = String::new();
    let continuation_prompt = "...> ";

    loop {
        let current_prompt = if buffer.is_empty() { &prompt } else { continuation_prompt };
        
        match editor.readline(current_prompt) {
            Ok(line) => {
                let trimmed = line.trim();
                
                // Empty line in continuation mode: execute buffer as-is
                if trimmed.is_empty() && !buffer.is_empty() {
                    let command = buffer.trim().to_string();
                    buffer.clear();
                    let _ = editor.add_history_entry(&command);
                    let continue_session =
                        apply_command(&mut session, &command, &executor, &mut stdout)?;
                    maybe_refresh_completion(&command, &mut editor, &session, &executor);
                    if !continue_session {
                        break;
                    }
                    continue;
                }
                
                if trimmed.is_empty() {
                    continue;
                }

                // Handle refresh command (multiple variants for convenience)
                let is_refresh = trimmed.eq_ignore_ascii_case(r"\refresh") 
                    || trimmed.eq_ignore_ascii_case("/refresh")
                    || trimmed.eq_ignore_ascii_case("refresh")
                    || trimmed.to_lowercase() == "\\refresh"
                    || trimmed.to_lowercase() == "/refresh";
                    
                if buffer.is_empty() && is_refresh {
                    let (words, count) = build_completion_list_with_tables(&session, &executor, &mut stdout);
                    if let Some(helper) = editor.helper_mut() {
                        helper.update(words);
                    }
                    writeln!(&mut stdout, "completion refreshed: {} tables", count)?;
                    continue;
                }
                
                // Backslash commands execute immediately (no semicolon needed)
                if buffer.is_empty() && trimmed.starts_with('\\') {
                    let _ = editor.add_history_entry(trimmed);
                    let continue_session =
                        apply_command(&mut session, trimmed, &executor, &mut stdout)?;
                    if !continue_session {
                        break;
                    }
                    continue;
                }
                
                // Forward slash commands also execute immediately
                if buffer.is_empty() && trimmed.starts_with('/') && !trimmed.contains(' ') {
                    let _ = editor.add_history_entry(trimmed);
                    let continue_session =
                        apply_command(&mut session, trimmed, &executor, &mut stdout)?;
                    if !continue_session {
                        break;
                    }
                    continue;
                }

                // Built-in commands execute immediately
                if buffer.is_empty() && matches!(trimmed.to_lowercase().as_str(), "help" | "exit" | "quit") {
                    let _ = editor.add_history_entry(trimmed);
                    let continue_session =
                        apply_command(&mut session, trimmed, &executor, &mut stdout)?;
                    if !continue_session {
                        break;
                    }
                    continue;
                }

                // Append to buffer
                if !buffer.is_empty() {
                    buffer.push('\n');
                }
                buffer.push_str(&line);

                // Check if statement is complete (ends with ;)
                if trimmed.ends_with(';') {
                    let command = buffer.trim().to_string();
                    buffer.clear();
                    let _ = editor.add_history_entry(&command);
                    let continue_session =
                        apply_command(&mut session, &command, &executor, &mut stdout)?;
                    maybe_refresh_completion(&command, &mut editor, &session, &executor);
                    if !continue_session {
                        break;
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C: clear buffer and continue
                if !buffer.is_empty() {
                    buffer.clear();
                    writeln!(&mut stdout, "{}", db_shell_shell_messages::BUFFER_CLEARED)?;
                    continue;
                }
                break;
            }
            Err(ReadlineError::Eof) => break,
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
        // Built-in commands
        "help".to_string(),
        "exit".to_string(),
        "quit".to_string(),
        r"\q".to_string(),
        r"\d".to_string(),
        r"\ping".to_string(),
        r"\c".to_string(),
        r"\refresh".to_string(),
        "refresh".to_string(),
        // SQL Keywords (uppercase)
        "SELECT".to_string(),
        "INSERT".to_string(),
        "UPDATE".to_string(),
        "DELETE".to_string(),
        "FROM".to_string(),
        "INTO".to_string(),
        "VALUES".to_string(),
        "WHERE".to_string(),
        "AND".to_string(),
        "OR".to_string(),
        "NOT".to_string(),
        "IN".to_string(),
        "LIKE".to_string(),
        "BETWEEN".to_string(),
        "IS".to_string(),
        "NULL".to_string(),
        "ORDER".to_string(),
        "BY".to_string(),
        "ASC".to_string(),
        "DESC".to_string(),
        "LIMIT".to_string(),
        "OFFSET".to_string(),
        "GROUP".to_string(),
        "HAVING".to_string(),
        "JOIN".to_string(),
        "INNER".to_string(),
        "LEFT".to_string(),
        "RIGHT".to_string(),
        "OUTER".to_string(),
        "ON".to_string(),
        "AS".to_string(),
        "DISTINCT".to_string(),
        "COUNT".to_string(),
        "SUM".to_string(),
        "AVG".to_string(),
        "MIN".to_string(),
        "MAX".to_string(),
        "CREATE".to_string(),
        "TABLE".to_string(),
        "DROP".to_string(),
        "ALTER".to_string(),
        "ADD".to_string(),
        "COLUMN".to_string(),
        "INDEX".to_string(),
        "PRIMARY".to_string(),
        "KEY".to_string(),
        "FOREIGN".to_string(),
        "REFERENCES".to_string(),
        "UNIQUE".to_string(),
        "DEFAULT".to_string(),
        "CASCADE".to_string(),
        "SET".to_string(),
        "TRUNCATE".to_string(),
        "BEGIN".to_string(),
        "COMMIT".to_string(),
        "ROLLBACK".to_string(),
        "TRANSACTION".to_string(),
        "RETURNING".to_string(),
        "EXPLAIN".to_string(),
        "ANALYZE".to_string(),
        "VACUUM".to_string(),
        "TRUE".to_string(),
        "FALSE".to_string(),
        // SQL Keywords (lowercase for convenience)
        "select".to_string(),
        "insert".to_string(),
        "update".to_string(),
        "delete".to_string(),
        "from".to_string(),
        "into".to_string(),
        "values".to_string(),
        "where".to_string(),
        "and".to_string(),
        "or".to_string(),
        "not".to_string(),
        "in".to_string(),
        "like".to_string(),
        "order".to_string(),
        "by".to_string(),
        "limit".to_string(),
        "join".to_string(),
        "on".to_string(),
        "as".to_string(),
        "create".to_string(),
        "table".to_string(),
        "drop".to_string(),
        "alter".to_string(),
        "set".to_string(),
        "returning".to_string(),
    ];

    if let Some(session) = session {
        // Add engine names
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

/// Fetch table names from the database for completion
pub fn fetch_table_names(
    session: &DbShellSession,
    executor: &RuntimeExecutor,
    out: &mut dyn Write,
) -> Vec<String> {
    match executor.run(session.list_tables()) {
        Ok(tables) => {
            let names: Vec<String> = tables.into_iter().map(|t| t.name).collect();
            debug!("loaded {} tables for completion", names.len());
            names
        }
        Err(e) => {
            let _ = writeln!(out, "note: could not load tables for completion: {e}");
            debug!("failed to fetch tables for completion: {e}");
            Vec::new()
        }
    }
}

/// Build a comprehensive completion list including table names
/// Returns (completion_list, table_count)
pub fn build_completion_list_with_tables(
    session: &DbShellSession,
    executor: &RuntimeExecutor,
    out: &mut dyn Write,
) -> (Vec<String>, usize) {
    let mut words = completion_words(Some(session));
    
    // Fetch and add table names
    let tables = fetch_table_names(session, executor, out);
    let table_count = tables.len();
    
    for table in tables {
        if !words.contains(&table) {
            words.push(table.clone());
        }
    }
    
    words.sort();
    words.dedup();
    (words, table_count)
}

/// Refresh completion list (called after DDL commands)
pub fn refresh_completion(
    editor: &mut Editor<ListCompleter, DefaultHistory>,
    session: &DbShellSession,
    executor: &RuntimeExecutor,
) {
    // Use a sink for errors during refresh (we don't want to interrupt the user)
    let mut sink = std::io::sink();
    let (words, _) = build_completion_list_with_tables(session, executor, &mut sink);
    if let Some(helper) = editor.helper_mut() {
        helper.update(words);
    }
}

/// Check if command might have changed schema and refresh completion if needed
fn maybe_refresh_completion(
    command: &str,
    editor: &mut Editor<ListCompleter, DefaultHistory>,
    session: &DbShellSession,
    executor: &RuntimeExecutor,
) {
    let cmd_upper = command.to_uppercase();
    // Refresh completion after DDL commands
    if cmd_upper.contains("CREATE TABLE")
        || cmd_upper.contains("DROP TABLE")
        || cmd_upper.contains("ALTER TABLE")
        || cmd_upper.contains("CREATE INDEX")
        || cmd_upper.contains("DROP INDEX")
        || cmd_upper.contains("CREATE VIEW")
        || cmd_upper.contains("DROP VIEW")
    {
        debug!("refreshing completion after DDL command");
        refresh_completion(editor, session, executor);
    }
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
    _runtime: Option<Runtime>,
}

impl RuntimeExecutor {
    pub fn new() -> io::Result<Self> {
        if let Ok(handle) = Handle::try_current() {
            Ok(Self {
                handle,
                _runtime: None,
            })
        } else {
            let runtime = Runtime::new().map_err(|err| io::Error::other(err.to_string()))?;
            let handle = runtime.handle().clone();
            Ok(Self {
                handle,
                _runtime: Some(runtime),
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
