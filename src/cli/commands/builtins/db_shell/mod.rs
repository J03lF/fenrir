use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::completion::SimpleCompleter;
use crate::domain::db::{
    DbError, DbExecutionResult, DbResult, DbResultSet, DbTable, DbTableSchema,
};
use crate::services::{db_shell::DbShellSession, ServiceStatus};
use rustyline::history::DefaultHistory;
use rustyline::{error::ReadlineError, Editor};
use std::borrow::Cow;
use std::io::{self, Write};
use tokio::runtime::{Handle, Runtime};
use tokio::task;

const DETAILS: &[&str] = &[
    r"\c <engine> – Engine wechseln",
    r"\d [table] – Tabellen auflisten oder Schema anzeigen",
    r"\ping – Verbindung testen",
    "exit / \\q – Subshell verlassen",
    "Destruktive SQLs benötigen '--force' am Zeilenende",
];

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "db-shell",
        "Öffnet die Datenbank-Subshell",
        "db-shell",
        DETAILS,
        handle,
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
    let default_engine = service.default_engine();
    let engines = service
        .available_engines()
        .iter()
        .map(|engine| engine.to_string())
        .collect::<Vec<_>>();
    let engines_line = if engines.is_empty() {
        "keine konfigurierten Engines".to_string()
    } else {
        format!("verfügbar: {}", engines.join(", "))
    };
    match env {
        ShellEnvironment::Cli => {
            deps.services.registry().set_status(
                "db-shell",
                ServiceStatus::Active,
                Some("DB-Shell (CLI) aktiv".to_string()),
            );
            writeln!(
                out,
                "Starte DB-Shell (Standard: {default_engine}) – {engines_line}"
            )?;
        }
        ShellEnvironment::Ssh => {
            deps.services.registry().set_status(
                "db-shell",
                ServiceStatus::Active,
                Some("DB-Shell (SSH) aktiv".to_string()),
            );
            writeln!(
                out,
                "Wechsle in DB-Shell (Standard: {default_engine}) – {engines_line}"
            )?;
        }
    }
    Ok(CommandOutcome::EnterDbShell)
}

pub fn run_local_db_shell(mut session: DbShellSession, prompt: String) -> io::Result<()> {
    let mut stdout = io::stdout();
    writeln!(
        &mut stdout,
        "DB-Shell aktiv. Aktueller Engine: {}",
        session.current_engine()
    )?;
    writeln!(&mut stdout, "Nutze 'help' für Übersicht der Befehle.")?;

    let mut editor =
        Editor::<SimpleCompleter, DefaultHistory>::new().map_err(map_readline_error)?;
    editor.set_helper(Some(SimpleCompleter::new(completion_words(Some(&session)))));
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
                writeln!(&mut stdout, "Eingabefehler: {err}")?;
                break;
            }
        }
    }
    Ok(())
}

pub(crate) fn completion_words(session: Option<&DbShellSession>) -> Vec<String> {
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

pub(crate) fn apply_command(
    session: &mut DbShellSession,
    command: &str,
    executor: &RuntimeExecutor,
    out: &mut dyn Write,
) -> io::Result<bool> {
    match process_command(session, command, executor) {
        Ok(CommandResult::Exit) => {
            writeln!(out, "DB-Shell beendet")?;
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
            writeln!(out, "Fehler: {}", sanitize_error(&err))?;
            Ok(true)
        }
    }
}

enum CommandResult {
    Exit,
    Message(Vec<String>),
    Execution(Vec<DbExecutionResult>),
    Tables(Vec<DbTable>),
    Schema(DbTableSchema),
}

fn process_command(
    session: &mut DbShellSession,
    command: &str,
    executor: &RuntimeExecutor,
) -> DbResult<CommandResult> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Ok(CommandResult::Message(vec![]));
    }
    if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case(r"\q") {
        return Ok(CommandResult::Exit);
    }
    if trimmed.eq_ignore_ascii_case("help") {
        return Ok(CommandResult::Message(help_lines(session)));
    }
    if trimmed.eq_ignore_ascii_case("/ping") {
        executor.run(session.ping())?;
        return Ok(CommandResult::Message(vec!["Ping erfolgreich".to_string()]));
    }
    if trimmed.starts_with("/c") {
        return handle_switch_command(session, trimmed, executor);
    }
    if trimmed == "/d" {
        let tables = executor.run(session.list_tables())?;
        return Ok(CommandResult::Tables(tables));
    }
    if trimmed.starts_with("/d ") {
        let table = trimmed[3..].trim();
        if table.is_empty() {
            return Ok(CommandResult::Message(vec![
                "Bitte Tabellenname angeben, z. B. \\d public.tickets".to_string(),
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
        return Ok(CommandResult::Message(vec![
            format!("Aktueller Engine: {}", session.current_engine()),
            format!("Verfügbare Engines: {}", engines.join(", ")),
            "Wechsel mit \\c <engine>".to_string(),
        ]));
    }
    let target = parts[1];
    let engine = target.parse()?;
    session.switch_engine(engine)?;
    executor.run(session.ping())?;
    Ok(CommandResult::Message(vec![format!(
        "Engine gewechselt zu {engine}"
    )]))
}

fn help_lines(session: &DbShellSession) -> Vec<String> {
    vec![
        "Meta-Befehle:".to_string(),
        "  \\c <engine>    – Engine wechseln".to_string(),
        "  \\d [table]    – Tabellen auflisten oder Schema anzeigen".to_string(),
        "  \\ping         – Verbindung testen".to_string(),
        "  exit / \\q    – DB-Shell verlassen".to_string(),
        "".to_string(),
        {
            let engines = session
                .available_engines()
                .iter()
                .map(|engine| engine.to_string())
                .collect::<Vec<_>>();
            format!(
                "Aktueller Engine: {} (verfügbar: {})",
                session.current_engine(),
                if engines.is_empty() {
                    "-".to_string()
                } else {
                    engines.join(", ")
                }
            )
        },
        "SQL-Befehle:".to_string(),
        "  Destruktive Befehle (DROP/DELETE/ALTER/TRUNCATE) benötigen '--force' am Zeilenende."
            .to_string(),
    ]
}

fn enforce_guard(input: &str) -> DbResult<String> {
    if !requires_guard(input) {
        return Ok(input.to_string());
    }
    let trimmed = input.trim_end();
    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with("--force") {
        let without_force = trimmed[..trimmed.len() - "--force".len()].trim_end();
        Ok(without_force.to_string())
    } else {
        Err(DbError::InvalidInput {
            message: "Destruktive Befehle erfordern '--force' am Zeilenende".to_string(),
        })
    }
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

fn render_execution_results(out: &mut dyn Write, results: &[DbExecutionResult]) -> io::Result<()> {
    if results.is_empty() {
        writeln!(out, "(keine Rückgabe)")?;
        return Ok(());
    }
    for result in results {
        match result {
            DbExecutionResult::ResultSet(set) => render_result_set(out, set)?,
            DbExecutionResult::AffectedRows(rows) => {
                writeln!(out, "{rows} Zeile(n) betroffen")?;
            }
            DbExecutionResult::CommandTag(tag) => {
                writeln!(out, "{tag}")?;
            }
        }
    }
    Ok(())
}

fn render_result_set(out: &mut dyn Write, set: &DbResultSet) -> io::Result<()> {
    if set.columns.is_empty() {
        writeln!(out, "(keine Spalten)")?;
        return Ok(());
    }
    let mut widths = set.columns.iter().map(|col| col.len()).collect::<Vec<_>>();
    for row in &set.rows {
        for (idx, value) in row.iter().enumerate() {
            let len = value.len();
            if len > widths[idx] {
                widths[idx] = len;
            }
        }
    }

    writeln!(out, "{}", build_separator(&widths))?;
    write!(out, "|")?;
    for (idx, column) in set.columns.iter().enumerate() {
        write!(out, " {:width$} |", column, width = widths[idx])?;
    }
    writeln!(out)?;
    writeln!(out, "{}", build_separator(&widths))?;
    for row in &set.rows {
        write!(out, "|")?;
        for (idx, value) in row.iter().enumerate() {
            write!(out, " {:width$} |", value, width = widths[idx])?;
        }
        writeln!(out)?;
    }
    writeln!(out, "{}", build_separator(&widths))?;
    writeln!(out, "({} Zeile(n))", set.rows.len())?;
    Ok(())
}

fn render_tables(out: &mut dyn Write, tables: &[DbTable]) -> io::Result<()> {
    if tables.is_empty() {
        writeln!(out, "Keine Tabellen gefunden")?;
        return Ok(());
    }
    for table in tables {
        let schema = table.schema.as_deref().unwrap_or("<unbekannt>");
        writeln!(out, "{schema}.{} ({:?})", table.name, table.kind)?;
    }
    Ok(())
}

fn render_schema(out: &mut dyn Write, schema: &DbTableSchema) -> io::Result<()> {
    writeln!(
        out,
        "Schema für {}.{}:",
        schema.table.schema.as_deref().unwrap_or("<unbekannt>"),
        schema.table.name
    )?;
    if schema.columns.is_empty() {
        writeln!(out, "  (keine Spalten)")?;
        return Ok(());
    }
    for column in &schema.columns {
        let nullable = if column.is_nullable {
            "NULL"
        } else {
            "NOT NULL"
        };
        let default = column
            .default_value
            .as_deref()
            .map(Cow::from)
            .unwrap_or(Cow::Borrowed(""));
        writeln!(
            out,
            "  {:<20} {:<20} {:<8} {}",
            column.name, column.data_type, nullable, default
        )?;
    }
    Ok(())
}

fn sanitize_error(err: &DbError) -> String {
    match err {
        DbError::EngineNotConfigured { engine } => {
            format!("Engine {engine} ist nicht konfiguriert")
        }
        DbError::Connection { .. } => "Verbindungsfehler".to_string(),
        DbError::Query { message } => message.clone(),
        DbError::InvalidInput { message } => message.clone(),
        DbError::NotImplemented { message } => message.clone(),
    }
}

fn build_separator(widths: &[usize]) -> String {
    let mut sep = String::from("+");
    for width in widths {
        let segment = "-".repeat(width + 2);
        sep.push_str(&segment);
        sep.push('+');
    }
    sep
}

fn map_readline_error(err: ReadlineError) -> io::Error {
    match err {
        ReadlineError::Io(err) => err,
        other => io::Error::new(io::ErrorKind::Other, other.to_string()),
    }
}

pub(crate) struct RuntimeExecutor {
    handle: Handle,
    #[allow(dead_code)]
    runtime: Option<Runtime>,
}

impl RuntimeExecutor {
    pub(crate) fn new() -> io::Result<Self> {
        if let Ok(handle) = Handle::try_current() {
            Ok(Self {
                handle,
                runtime: None,
            })
        } else {
            let runtime = Runtime::new()
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
            let handle = runtime.handle().clone();
            Ok(Self {
                handle,
                runtime: Some(runtime),
            })
        }
    }

    pub(crate) fn run<T>(
        &self,
        future: impl std::future::Future<Output = DbResult<T>>,
    ) -> DbResult<T> {
        if Handle::try_current().is_ok() {
            task::block_in_place(|| self.handle.block_on(future))
        } else {
            self.handle.block_on(future)
        }
    }
}

mod guard_tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn guard_detection() {
        assert!(requires_guard("drop table"));
        assert!(requires_guard("DELETE FROM foo"));
        assert!(!requires_guard("select * from foo"));
    }

    #[test]
    fn enforce_guard_allows_force() {
        let stmt = "DROP TABLE foo; --force";
        let sanitized = enforce_guard(stmt).expect("force should allow");
        assert!(sanitized.contains("DROP TABLE"));
        assert!(!sanitized.contains("--force"));
    }

    #[test]
    fn enforce_guard_blocks_without_force() {
        assert!(enforce_guard("DROP TABLE foo").is_err());
    }
}
