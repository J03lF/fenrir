use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandOutput, CommandRegistry,
    CommandShape, CompletionContext, CompletionKind, ShellEnvironment,
};
use crate::cli::output::{FieldStyle, MessageBox, StatusBox, SYM_SUCCESS};
use crate::config::AppConfig;
use crate::domain::db::DbEngine;
use crate::services::AppServices;

use super::export::{parse_engine, run_export_with_progress, ExportMode, ExportProgress};
use super::import::{run_import_with_progress, ImportProgress};

const EXPORT_ARGS: &[CommandArgument] = &[
    CommandArgument::required("target"), // "all", "table <name>", or file path
    CommandArgument::optional("name_or_engine"),
    CommandArgument::optional("engine").with_completion(CompletionKind::Static(&[
        "postgres", "sqlite", "mysql", "mongodb",
    ])),
];
const IMPORT_ARGS: &[CommandArgument] = &[
    CommandArgument::required("file"),
    CommandArgument::optional("engine").with_completion(CompletionKind::Static(&[
        "postgres", "sqlite", "mysql", "mongodb",
    ])),
    CommandArgument::optional("--dry-run"),
    CommandArgument::optional("--force"),
];

pub fn command() -> CommandEntry {
    const SUBS: &[crate::cli::commands::registry::CommandSubcommand] = &[
        crate::cli::commands::registry::CommandSubcommand::new(
            "export",
            &["exp"],
            EXPORT_ARGS,
            "Export DB schema to StarUML (.mdj)",
        ),
        crate::cli::commands::registry::CommandSubcommand::new(
            "import",
            &["imp"],
            IMPORT_ARGS,
            "Import StarUML (.mdj) and apply migrations",
        ),
    ];
    let shape = CommandShape::new("db schema", &[], &[], SUBS);
    CommandEntry::with_shape(
        "db schema",
        "DB schema utilities",
        "db schema <subcommand>",
        &["Manage and export DB schema"],
        handle,
        shape,
    )
}

pub fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let sub = args.first().copied().unwrap_or("export");
    let tail = &args[1..];

    match sub {
        "export" | "exp" => {
            if tail.is_empty() {
                writeln!(
                    out,
                    "usage:\n  db schema export all              - export all tables\n  \
                     db schema export table <name>      - export specific table\n  \
                     db schema export <file> [engine]   - export to file\n\n\
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

            let Some(sink) = deps.output() else {
                MessageBox::error("Output not available")
                    .message("Cannot stream output")
                    .render(out)?;
                return Ok(CommandOutcome::Continue);
            };

            let services = Arc::clone(&deps.services);
            let config = Arc::clone(&deps.config);
            let future = async move { execute_export(sink, services, config, engine, mode).await };

            Ok(CommandOutcome::AsyncTask(Box::pin(future)))
        }
        "import" | "imp" => {
            if tail.is_empty() {
                writeln!(
                    out,
                    "usage: db schema import <file> [engine] [--dry-run] [--force]"
                )?;
                return Ok(CommandOutcome::Continue);
            }

            let path = resolve_schema_path(&deps.config, tail[0]);
            let engine = parse_engine(tail.get(1).filter(|v| !v.starts_with("--")).copied());
            let dry_run = tail.contains(&"--dry-run");
            let force = tail.contains(&"--force");

            if !path.exists() {
                MessageBox::error("File not found")
                    .message(format!("Schema file does not exist: {}", path.display()))
                    .render(out)?;
                return Ok(CommandOutcome::Continue);
            }

            let Some(sink) = deps.output() else {
                MessageBox::error("Output not available")
                    .message("Cannot stream output")
                    .render(out)?;
                return Ok(CommandOutcome::Continue);
            };

            let services = Arc::clone(&deps.services);
            let config = Arc::clone(&deps.config);
            let future = async move {
                execute_import(sink, services, config, path, engine, dry_run, force).await
            };

            Ok(CommandOutcome::AsyncTask(Box::pin(future)))
        }
        other => {
            writeln!(out, "unknown subcommand {other}")?;
            Ok(CommandOutcome::Continue)
        }
    }
}

pub fn completion(ctx: &CompletionContext<'_>) -> Vec<String> {
    let subs = ["export", "exp", "import", "imp"];
    match ctx.active_index {
        0 => subs
            .iter()
            .filter(|s| s.starts_with(ctx.prefix))
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
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

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<ExportProgress>();

    let export_task = tokio::spawn(async move {
        let callback = move |progress: ExportProgress| {
            let _ = progress_tx.send(progress);
        };
        run_export_with_progress(&services, &config, engine, mode, Some(callback)).await
    });

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
                .field_styled(
                    "Status",
                    format!("{} Success", SYM_SUCCESS),
                    FieldStyle::Success,
                )
                .field("Tables", export_result.table_count.to_string());

            status = status
                .section()
                .field("Path", export_result.path.to_string_lossy());

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

async fn execute_import(
    sink: Arc<dyn CommandOutput>,
    services: Arc<AppServices>,
    config: Arc<AppConfig>,
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

    if !dry_run && config.db.schema.backup_before_import {
        if let Some(backup_service) = services.backup_service() {
            if backup_service.is_enabled() {
                match backup_service
                    .run_backup(crate::services::backup::BackupTrigger::Manual)
                    .await
                {
                    Ok(status) => {
                        writeln!(
                            out,
                            "  \x1b[38;5;81m▸\x1b[0m Backup created at {}",
                            status.path.display()
                        )?;
                    }
                    Err(err) => {
                        MessageBox::error("Backup Failed")
                            .message(err.to_string())
                            .render(&mut out)?;
                        return Ok(CommandOutcome::Continue);
                    }
                }
            }
        }
    }

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<ImportProgress>();

    let services_clone = Arc::clone(&services);
    let path_clone = path.clone();
    let import_task = tokio::spawn(async move {
        let callback = move |progress: ImportProgress| {
            let _ = progress_tx.send(progress);
        };
        run_import_with_progress(
            &services_clone,
            path_clone,
            engine,
            dry_run,
            force,
            Some(callback),
        )
        .await
    });

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

            let mut status = StatusBox::new("Import Complete").field_styled(
                "Status",
                format!("{} Success", SYM_SUCCESS),
                FieldStyle::Success,
            );

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

fn resolve_schema_path(config: &AppConfig, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        return path;
    }
    if path.exists() {
        return path;
    }
    let import_dir = PathBuf::from(&config.db.schema.import_dir);
    let candidate = import_dir.join(&path);
    if candidate.exists() {
        candidate
    } else {
        path
    }
}
