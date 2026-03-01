use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandOutput, CommandRegistry,
    CommandShape, CommandSubcommand, ShellEnvironment,
};
use crate::cli::output::{FieldStyle, MessageBox, StatusBox, SYM_SUCCESS};
use crate::infra::db::runtime::{DbRuntimeSupervisor, RestoreProgress};

const DB_ARGS: &[CommandArgument] = &[CommandArgument::required("backup_file")];

const RESTORE_SUBCOMMANDS: &[CommandSubcommand] = &[CommandSubcommand::new(
    "db",
    &["database"],
    DB_ARGS,
    "Restore embedded database from backup",
)];

const RESTORE_SHAPE: CommandShape = CommandShape::new("restore", &[], &[], RESTORE_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "restore",
        "Restore resources",
        "restore <resource> <backup_file>",
        &["Restore db, config, or other resources from backup"],
        handle,
        RESTORE_SHAPE,
    )
}

fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let sub = args.first().copied().unwrap_or("db");
    let tail = if args.is_empty() { args } else { &args[1..] };

    match sub {
        "db" | "database" => handle_db_restore(deps, tail, out),
        other => {
            MessageBox::error("Unknown restore target")
                .message(format!("'{}' is not a valid restore target", other))
                .suggestion("Valid targets: db")
                .render(out)?;
            Ok(CommandOutcome::Continue)
        }
    }
}

fn handle_db_restore(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some(backup_file) = args.first() else {
        MessageBox::error("Missing backup file")
            .message("Please specify the backup file to restore from")
            .suggestion("Usage: restore db <backup_file>")
            .render(out)?;
        return Ok(CommandOutcome::Continue);
    };

    let Some(rt) = deps.services.db_runtime() else {
        MessageBox::error("DB Runtime not available")
            .message("Embedded database runtime is not enabled")
            .render(out)?;
        return Ok(CommandOutcome::Continue);
    };

    // Check if backup exists
    let backup_path = PathBuf::from(*backup_file);
    if !backup_path.exists() {
        MessageBox::error("Backup not found")
            .message(format!("File does not exist: {}", backup_file))
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

    // Return async task
    let backup_path_owned = backup_path.clone();
    let rt_clone = Arc::clone(&rt);
    let future = async move { execute_restore(sink, rt_clone, backup_path_owned).await };

    Ok(CommandOutcome::AsyncTask(Box::pin(future)))
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

async fn execute_restore(
    sink: Arc<dyn CommandOutput>,
    rt: Arc<DbRuntimeSupervisor>,
    backup_path: PathBuf,
) -> io::Result<CommandOutcome> {
    let mut out = StreamedWriter::new(sink);

    writeln!(out)?;
    writeln!(
        out,
        "  \x1b[38;5;81m▸\x1b[0m Restoring database from backup..."
    )?;
    writeln!(out)?;

    // Create progress channel
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<RestoreProgress>();

    // Clone for the restore task
    let rt_for_task = Arc::clone(&rt);
    let backup_str = backup_path.to_string_lossy().to_string();

    // Spawn the restore task
    let restore_task = tokio::spawn(async move {
        let callback = move |progress: RestoreProgress| {
            let _ = progress_tx.send(progress);
        };
        rt_for_task
            .restore_with_progress(&backup_str, Some(callback))
            .await
    });

    // Run select loop for live progress
    let mut last_step = 0u8;
    let mut restore_task = restore_task;

    let result = loop {
        tokio::select! {
            biased;
            Some(progress) = progress_rx.recv() => {
                match progress {
                    RestoreProgress::StoppingServer if last_step < 1 => {
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Stopping database server...")?;
                        last_step = 1;
                    }
                    RestoreProgress::ServerStopped if last_step < 2 => {
                        write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Stopping database server   \n")?;
                        last_step = 2;
                    }
                    RestoreProgress::RestoringData if last_step < 3 => {
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Restoring data from backup...")?;
                        last_step = 3;
                    }
                    RestoreProgress::DataRestored if last_step < 4 => {
                        write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Restoring data from backup   \n")?;
                        last_step = 4;
                    }
                    RestoreProgress::CleaningUp if last_step < 5 => {
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Cleaning up recovery files...")?;
                        last_step = 5;
                    }
                    RestoreProgress::CleanupComplete if last_step < 6 => {
                        write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Cleaning up recovery files   \n")?;
                        last_step = 6;
                    }
                    RestoreProgress::StartingServer if last_step < 7 => {
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Starting database server...")?;
                        last_step = 7;
                    }
                    RestoreProgress::ServerStarted if last_step < 8 => {
                        write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Starting database server   \n")?;
                        last_step = 8;
                    }
                    RestoreProgress::Complete => {
                        // Done
                    }
                    _ => {}
                }
            }
            result = &mut restore_task => {
                break result;
            }
        }
    };

    match result {
        Ok(Ok(())) => {
            writeln!(out)?;

            // Show success box
            let mut status = StatusBox::new("Restore Complete").field_styled(
                "Status",
                format!("{} Success", SYM_SUCCESS),
                FieldStyle::Success,
            );

            // Get backup size
            if let Ok(metadata) = std::fs::metadata(&backup_path) {
                if metadata.is_dir() {
                    let size = calculate_dir_size(&backup_path);
                    status = status.field("Size", format_size(size));
                } else {
                    status = status.field("Size", format_size(metadata.len()));
                }
            }

            // Put full backup path in its own section
            status = status
                .section()
                .field("Source", backup_path.to_string_lossy());

            status.render(&mut out)?;
        }
        Ok(Err(err)) => {
            writeln!(out)?;
            writeln!(out, "    \x1b[38;5;203m✗\x1b[0m    Operation failed")?;
            writeln!(out)?;

            MessageBox::error("Restore Failed")
                .message(err.to_string())
                .suggestion("Check the backup file and try again")
                .render(&mut out)?;
        }
        Err(err) => {
            writeln!(out)?;
            MessageBox::error("Restore Failed")
                .message(format!("Task error: {}", err))
                .render(&mut out)?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn calculate_dir_size(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() {
                        calculate_dir_size(&p)
                    } else {
                        e.metadata().map(|m| m.len()).unwrap_or(0)
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}
