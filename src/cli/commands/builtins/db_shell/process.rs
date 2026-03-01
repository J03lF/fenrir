use crate::domain::db::{DbError, DbExecutionResult, DbResult, DbTable, DbTableSchema};
use crate::services::db_shell::{DbShellSession, DESTRUCTIVE_FORCE_WARNING};
use crate::utils::messages::cli::builtins::db_shell::{
    errors as db_shell_error_messages, process as db_shell_process_messages,
};

use super::shell::RuntimeExecutor;

pub enum CommandResult {
    Exit,
    Message(Vec<String>),
    Execution(Vec<DbExecutionResult>),
    Tables(Vec<DbTable>),
    Schema(DbTableSchema),
}

pub fn process_command(
    session: &mut DbShellSession,
    command: &str,
    executor: &RuntimeExecutor,
) -> DbResult<CommandResult> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Ok(CommandResult::Message(vec![]));
    }
    let normalized = normalize_meta_command(trimmed);
    if normalized.eq_ignore_ascii_case("exit") || normalized.eq_ignore_ascii_case(r"\q") {
        return Ok(CommandResult::Exit);
    }
    if normalized.eq_ignore_ascii_case("help") {
        return Ok(CommandResult::Message(help_lines(session)));
    }
    if normalized.eq_ignore_ascii_case("/ping") || normalized.eq_ignore_ascii_case(r"\ping") {
        executor.run(session.ping())?;
        return Ok(CommandResult::Message(vec![
            db_shell_process_messages::PING_OK.to_string(),
        ]));
    }
    // \refresh is handled in shell.rs, but return a hint if it reaches here
    if normalized.eq_ignore_ascii_case("/refresh") || normalized.eq_ignore_ascii_case(r"\refresh") {
        return Ok(CommandResult::Message(vec![
            "Use \\refresh at start of line (not after other input)".to_string(),
        ]));
    }
    if normalized.starts_with("/c") || normalized.starts_with(r"\c") {
        return handle_switch_command(session, normalized, executor);
    }
    if normalized == "/d" || normalized == r"\d" {
        let tables = executor.run(session.list_tables())?;
        return Ok(CommandResult::Tables(tables));
    }
    // Describe table: /d <table> or \d <table>
    let describe_prefix = if normalized.starts_with("/d ") {
        Some("/d ")
    } else if normalized.starts_with(r"\d ") {
        Some(r"\d ")
    } else {
        None
    };
    if let Some(prefix) = describe_prefix {
        let table = normalized.strip_prefix(prefix).unwrap_or("").trim();
        if table.is_empty() {
            return Ok(CommandResult::Message(vec![
                db_shell_process_messages::TABLE_NAME_HINT.to_string(),
            ]));
        }
        let schema = executor.run(session.describe_table(table))?;
        return Ok(CommandResult::Schema(schema));
    }

    let statement = enforce_guard(trimmed)?;
    let results = executor.run(session.simple_query(&statement))?;
    Ok(CommandResult::Execution(results))
}

fn handle_switch_command(
    session: &mut DbShellSession,
    command: &str,
    executor: &RuntimeExecutor,
) -> DbResult<CommandResult> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.len() == 1 {
        let engines = session
            .available_engines()
            .iter()
            .map(|engine| engine.to_string())
            .collect::<Vec<_>>();
        let current_engine =
            db_shell_process_messages::current_engine(&session.current_engine().to_string());
        let available = db_shell_process_messages::available_engines(&engines);
        return Ok(CommandResult::Message(vec![
            current_engine,
            available,
            db_shell_process_messages::SWITCH_PROMPT.to_string(),
        ]));
    }
    let target = parts[1];
    let engine = target.parse()?;
    session.switch_engine(engine)?;
    executor.run(session.ping())?;
    Ok(CommandResult::Message(vec![
        db_shell_process_messages::engine_switched(&engine.to_string()),
    ]))
}

fn help_lines(session: &DbShellSession) -> Vec<String> {
    let engines = session
        .available_engines()
        .iter()
        .map(|engine| engine.to_string())
        .collect::<Vec<_>>();
    let options = if engines.is_empty() {
        "-".to_string()
    } else {
        engines.join(", ")
    };
    vec![
        db_shell_process_messages::META_HEADER.to_string(),
        db_shell_process_messages::META_SWITCH.to_string(),
        db_shell_process_messages::META_TABLES.to_string(),
        db_shell_process_messages::META_PING.to_string(),
        db_shell_process_messages::META_REFRESH.to_string(),
        db_shell_process_messages::META_EXIT.to_string(),
        String::new(),
        db_shell_process_messages::meta_engine_line(
            &session.current_engine().to_string(),
            &options,
        ),
        db_shell_process_messages::SQL_HEADER.to_string(),
        db_shell_process_messages::guard_instruction(DESTRUCTIVE_FORCE_WARNING),
    ]
}

fn enforce_guard(input: &str) -> DbResult<String> {
    if !requires_guard(input) {
        return Ok(input.to_string());
    }
    let trimmed = input.trim_end();
    let trimmed_no_semicolons = trimmed.trim_end_matches(';').trim_end();
    let lower = trimmed_no_semicolons.to_ascii_lowercase();
    if lower.ends_with("--force") {
        let force_start = trimmed_no_semicolons.len() - "--force".len();
        let prefix = &trimmed_no_semicolons[..force_start];
        let has_boundary = if force_start == 0 {
            true
        } else {
            prefix
                .chars()
                .last()
                .map(|ch| ch.is_whitespace())
                .unwrap_or(false)
        };
        if has_boundary {
            let without_force = prefix.trim_end();
            return Ok(without_force.to_string());
        }
    }
    Err(DbError::InvalidInput {
        message: DESTRUCTIVE_FORCE_WARNING.to_string(),
    })
}

fn requires_guard(statement: &str) -> bool {
    let normalized = statement.trim_start_matches('(').trim_start();
    if let Some(first) = normalized.split_whitespace().next() {
        matches!(
            first.to_ascii_uppercase().as_str(),
            "DROP" | "DELETE" | "ALTER" | "TRUNCATE"
        )
    } else {
        false
    }
}

fn normalize_meta_command(input: &str) -> &str {
    // First trim whitespace, then remove trailing semicolons, then trim again
    input.trim().trim_end_matches(';').trim()
}

pub fn sanitize_error(err: &DbError) -> String {
    match err {
        DbError::EngineNotConfigured { engine } => {
            db_shell_error_messages::engine_not_configured(&engine.to_string())
        }
        DbError::Connection { .. } => db_shell_error_messages::CONNECTION.to_string(),
        DbError::Query { message } => message.clone(),
        DbError::InvalidInput { message } => message.clone(),
        DbError::NotImplemented { message } => message.clone(),
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/cli/commands/builtins/db_shell/process_tests.rs"]
mod guard_tests;
