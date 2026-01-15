use std::io::{self, Write};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::cli::commands::builtins::db_schema::export::{
    parse_engine, run_export_with_progress, ExportMode, ExportProgress,
};
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandOutput, CommandRegistry,
    CommandShape, CommandSubcommand, CompletionKind, ShellEnvironment,
};
use crate::cli::output::{FieldStyle, MessageBox, StatusBox, SYM_SUCCESS};
use crate::config::AppConfig;
use crate::domain::db::DbEngine;
use crate::services::AppServices;

const SCHEMA_ARGS: &[CommandArgument] = &[
    CommandArgument::required("target"), // "all", "table <name>", or file path
    CommandArgument::optional("name_or_engine"),
    CommandArgument::optional("engine")
        .with_completion(CompletionKind::Static(&["postgres", "sqlite"])),
];

const EXPORT_SUBCOMMANDS: &[CommandSubcommand] = &[CommandSubcommand::new(
    "schema",
    &["db"],
    SCHEMA_ARGS,
    "Export database schema to StarUML (.mdj)",
)];

const EXPORT_SHAPE: CommandShape = CommandShape::new("export", &[], &[], EXPORT_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "export",
        "Export resources",
        "export <resource> [options]",
        &["Export schema, data, or other resources"],
        handle,
        EXPORT_SHAPE,
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

    match sub {
        "schema" | "db" => {
            if tail.is_empty() {
                writeln!(
                    out,
                    "usage:\n  export schema all              - export all tables\n  \
                     export schema table <name>      - export specific table\n  \
                     export schema <file> [engine]   - export to file\n\n\
                     export dir: {}",
                    deps.config.db.schema.export_dir
                )?;
                return Ok(CommandOutcome::Continue);
            }

            let mode = ExportMode::parse(tail);
            let engine = match tail.first().copied() {
                Some("all") => parse_engine(tail.get(1).copied()),
                Some("table") => parse_engine(tail.get(2).copied()),
                _ => parse_engine(tail.get(1).copied()),
            };

            let Some(mode) = mode else {
                writeln!(out, "invalid export arguments")?;
                return Ok(CommandOutcome::Continue);
            };

            // Get output sink for async task
            let Some(sink) = deps.output() else {
                MessageBox::error("Output not available")
                    .message("Cannot stream output")
                    .render(out)?;
                return Ok(CommandOutcome::Continue);
            };

            let services = Arc::clone(&deps.services);
            let config = Arc::clone(&deps.config);
            let future =
                async move { execute_export(sink, services, config, engine, mode).await };

            Ok(CommandOutcome::AsyncTask(Box::pin(future)))
        }
        other => {
            writeln!(out, "unknown export target: {other}\nvalid: schema")?;
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

async fn execute_export(
    sink: Arc<dyn CommandOutput>,
    services: Arc<AppServices>,
    config: Arc<AppConfig>,
    engine: Option<DbEngine>,
    mode: ExportMode,
) -> io::Result<CommandOutcome> {
    let mut out = StreamedWriter::new(sink);

    let mode_desc = match &mode {
        ExportMode::All => "all tables".to_string(),
        ExportMode::Table(name) => format!("table '{}'", name),
        ExportMode::File(path) => format!("to {}", path.display()),
    };

    writeln!(out)?;
    writeln!(
        out,
        "  \x1b[38;5;81m▸\x1b[0m Exporting schema ({})...",
        mode_desc
    )?;
    writeln!(out)?;

    // Create progress channel
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<ExportProgress>();

    // Spawn the export task
    let export_task = tokio::spawn(async move {
        let callback = move |progress: ExportProgress| {
            let _ = progress_tx.send(progress);
        };
        run_export_with_progress(&services, &config, engine, mode, Some(callback)).await
    });

    // Run select loop for live progress
    let mut last_step = 0u8;
    let mut export_task = export_task;

    let result = loop {
        tokio::select! {
            biased;
            Some(progress) = progress_rx.recv() => {
                match progress {
                    ExportProgress::PreparingDirectory if last_step < 1 => {
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Preparing export directory...")?;
                        last_step = 1;
                    }
                    ExportProgress::SnapshotDatabase if last_step < 2 => {
                        if last_step >= 1 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Preparing export directory   \n")?;
                        }
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Reading database schema...")?;
                        last_step = 2;
                    }
                    ExportProgress::FilteringTables if last_step < 3 => {
                        if last_step >= 2 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Reading database schema    \n")?;
                        }
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Filtering tables...")?;
                        last_step = 3;
                    }
                    ExportProgress::WritingFile if last_step < 4 => {
                        if last_step == 3 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Filtering tables           \n")?;
                        } else if last_step >= 2 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Reading database schema    \n")?;
                        }
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Writing StarUML file...")?;
                        last_step = 4;
                    }
                    ExportProgress::Complete => {
                        if last_step >= 4 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Writing StarUML file       \n")?;
                        }
                    }
                    _ => {}
                }
            }
            result = &mut export_task => {
                break result;
            }
        }
    };

    match result {
        Ok(Ok(export_result)) => {
            writeln!(out)?;

            let mut status = StatusBox::new("Export Complete")
                .field_styled("Status", format!("{} Success", SYM_SUCCESS), FieldStyle::Success)
                .field("Tables", export_result.table_count.to_string());

            status = status.section().field("Path", export_result.path.to_string_lossy());

            status.render(&mut out)?;
        }
        Ok(Err(err)) => {
            writeln!(out)?;
            writeln!(out, "    \x1b[38;5;203m✗\x1b[0m    Operation failed")?;
            writeln!(out)?;

            MessageBox::error("Export Failed")
                .message(err)
                .render(&mut out)?;
        }
        Err(err) => {
            writeln!(out)?;
            MessageBox::error("Export Failed")
                .message(format!("Task error: {}", err))
                .render(&mut out)?;
        }
    }

    Ok(CommandOutcome::Continue)
}
