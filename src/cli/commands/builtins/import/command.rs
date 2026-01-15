use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::cli::commands::builtins::db_schema::export::parse_engine;
use crate::cli::commands::builtins::db_schema::import::{run_import_with_progress, ImportProgress};
use crate::cli::commands::builtins::modules;
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandOutput, CommandRegistry,
    CommandShape, CommandSubcommand, CompletionKind, ShellEnvironment,
};
use crate::cli::output::{FieldStyle, MessageBox, StatusBox, SYM_SUCCESS};
use crate::domain::db::DbEngine;
use crate::services::AppServices;

const SCHEMA_ARGS: &[CommandArgument] = &[
    CommandArgument::required("file"),
    CommandArgument::optional("engine")
        .with_completion(CompletionKind::Static(&["postgres", "sqlite"])),
    CommandArgument::optional("--dry-run"),
    CommandArgument::optional("--force"),
];

const IMPORT_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new(
        "schema",
        &["db"],
        SCHEMA_ARGS,
        "Import StarUML schema and apply migrations",
    ),
    CommandSubcommand::new(
        "distribution",
        &["distributions"],
        &[],
        "Install Fenrir module distribution (alias to modules install distribution)",
    ),
];

const IMPORT_SHAPE: CommandShape = CommandShape::new("import", &[], &[], IMPORT_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "import",
        "Import resources",
        "import <resource> <file> [options]",
        &["Import schema, data, or other resources"],
        handle,
        IMPORT_SHAPE,
    )
}

fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let sub = args.first().copied().unwrap_or("schema");
    let tail = if args.is_empty() { args } else { &args[1..] };

    if matches!(sub, "distribution" | "distributions") {
        return modules::run_module_command(deps, "install-distribution", tail, out);
    }

    match sub {
        "schema" | "db" => {
            if tail.is_empty() {
                writeln!(
                    out,
                    "usage: import schema <file> [engine] [--dry-run] [--force]"
                )?;
                return Ok(CommandOutcome::Continue);
            }

            let path = PathBuf::from(tail[0]);
            let engine = parse_engine(tail.get(1).filter(|v| !v.starts_with("--")).copied());
            let dry_run = tail.contains(&"--dry-run");
            let force = tail.contains(&"--force");

            // Check file exists
            if !path.exists() {
                MessageBox::error("File not found")
                    .message(format!("Schema file does not exist: {}", path.display()))
                    .render(out)?;
                return Ok(CommandOutcome::Continue);
            }

            // Get output sink for async task
            let Some(sink) = deps.output() else {
                MessageBox::error("Output not available")
                    .message("Cannot stream output")
                    .render(out)?;
                return Ok(CommandOutcome::Continue);
            };

            let services = Arc::clone(&deps.services);
            let future = async move {
                execute_import(sink, services, path, engine, dry_run, force).await
            };

            Ok(CommandOutcome::AsyncTask(Box::pin(future)))
        }
        other => {
            writeln!(out, "unknown import target: {other}\nvalid: schema, distribution")?;
            Ok(CommandOutcome::Continue)
        }
    }
}

/// Streamed writer for async output
struct StreamedWriter {
    sink: Arc<dyn CommandOutput>,
}

impl StreamedWriter {
    fn new(sink: Arc<dyn CommandOutput>) -> Self {
        Self { sink }
    }
}

impl Write for StreamedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sink.push(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

async fn execute_import(
    sink: Arc<dyn CommandOutput>,
    services: Arc<AppServices>,
    path: PathBuf,
    engine: Option<DbEngine>,
    dry_run: bool,
    force: bool,
) -> io::Result<CommandOutcome> {
    let mut out = StreamedWriter::new(sink);

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    writeln!(out)?;
    writeln!(
        out,
        "  \x1b[38;5;81m▸\x1b[0m Importing schema from {}...",
        file_name
    )?;
    writeln!(out)?;

    // Create progress channel
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<ImportProgress>();

    // Spawn the import task
    let services_clone = Arc::clone(&services);
    let path_clone = path.clone();
    let import_task = tokio::spawn(async move {
        let callback = move |progress: ImportProgress| {
            let _ = progress_tx.send(progress);
        };
        run_import_with_progress(&services_clone, path_clone, engine, dry_run, force, Some(callback))
            .await
    });

    // Run select loop for live progress
    let mut last_step = 0u8;
    let mut total_stmts = 0usize;
    let mut import_task = import_task;

    let result = loop {
        tokio::select! {
            biased;
            Some(progress) = progress_rx.recv() => {
                match progress {
                    ImportProgress::ReadingFile if last_step < 1 => {
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Reading schema file...")?;
                        last_step = 1;
                    }
                    ImportProgress::ParsingSchema if last_step < 2 => {
                        if last_step >= 1 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Reading schema file       \n")?;
                        }
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Parsing StarUML schema...")?;
                        last_step = 2;
                    }
                    ImportProgress::SnapshotDatabase if last_step < 3 => {
                        if last_step >= 2 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Parsing StarUML schema    \n")?;
                        }
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Reading current database...")?;
                        last_step = 3;
                    }
                    ImportProgress::ComparingSchemas if last_step < 4 => {
                        if last_step >= 3 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Reading current database   \n")?;
                        }
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Comparing schemas...")?;
                        last_step = 4;
                    }
                    ImportProgress::PlanningMigrations if last_step < 5 => {
                        if last_step >= 4 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Comparing schemas          \n")?;
                        }
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Planning migrations...")?;
                        last_step = 5;
                    }
                    ImportProgress::ApplyingStatement(current, total) => {
                        if last_step < 6 {
                            if last_step >= 5 {
                                write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Planning migrations        \n")?;
                            }
                            last_step = 6;
                        }
                        total_stmts = total;
                        write!(out, "\r    \x1b[38;5;250m◦\x1b[0m    Applying migrations ({}/{})...", current, total)?;
                    }
                    ImportProgress::Complete => {
                        if last_step == 6 && total_stmts > 0 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Applying migrations ({}/{})   \n", total_stmts, total_stmts)?;
                        } else if last_step >= 5 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Planning migrations        \n")?;
                        }
                    }
                    _ => {}
                }
            }
            result = &mut import_task => {
                break result;
            }
        }
    };

    match result {
        Ok(Ok(import_result)) => {
            writeln!(out)?;

            let mut status = StatusBox::new("Import Complete")
                .field_styled("Status", format!("{} Success", SYM_SUCCESS), FieldStyle::Success);

            if import_result.dry_run {
                status = status.field("Mode", "dry-run");
            }

            if import_result.statements_count > 0 {
                status = status.field("Statements", import_result.statements_count.to_string());
            }

            status = status.section().field("File", path.to_string_lossy());

            status.render(&mut out)?;
        }
        Ok(Err(err)) => {
            writeln!(out)?;
            writeln!(out, "    \x1b[38;5;203m✗\x1b[0m    Operation failed")?;
            writeln!(out)?;

            MessageBox::error("Import Failed")
                .message(err)
                .render(&mut out)?;
        }
        Err(err) => {
            writeln!(out)?;
            MessageBox::error("Import Failed")
                .message(format!("Task error: {}", err))
                .render(&mut out)?;
        }
    }

    Ok(CommandOutcome::Continue)
}
