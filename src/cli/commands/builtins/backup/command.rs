use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::warn;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandOutput, CommandRegistry,
    CommandShape, CommandSubcommand, ShellEnvironment,
};
use crate::cli::output::{BoxTable, FieldStyle, MessageBox, StatusBox, SYM_ERROR, SYM_SUCCESS};
use crate::infra::db::runtime::{BackupProgress, DbRuntimeSupervisor};
use crate::services::AppServices;

const DB_ARGS: &[CommandArgument] = &[CommandArgument::optional("label|--list|--status")];

const BACKUP_SUBCOMMANDS: &[CommandSubcommand] = &[CommandSubcommand::new(
    "db",
    &["database"],
    DB_ARGS,
    "Backup embedded database (--list to show backups, --status for config)",
)];

const BACKUP_SHAPE: CommandShape = CommandShape::new("backup", &[], &[], BACKUP_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "backup",
        "Backup resources",
        "backup <resource> [options]",
        &[
            "Create backups of db, config, or other resources",
            "  backup db [label]  - Create backup with optional label",
            "  backup db --list   - List existing backups",
            "  backup db --status - Show backup configuration",
        ],
        handle,
        BACKUP_SHAPE,
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
        "db" | "database" => handle_db_backup(deps, tail, out),
        other => {
            MessageBox::error("Unknown backup target")
                .message(format!("'{}' is not a valid backup target", other))
                .suggestion("Valid targets: db")
                .render(out)?;
            Ok(CommandOutcome::Continue)
        }
    }
}

fn handle_db_backup(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    // Check for flags
    let first_arg = args.first().copied();

    match first_arg {
        Some("--list") | Some("-l") => {
            // List existing backups
            show_backup_list(deps, out)
        }
        Some("--status") | Some("-s") => {
            // Show backup config status
            show_backup_status(deps, out)
        }
        _ => {
            // Create backup
            let label = first_arg
                .filter(|s| !s.starts_with('-'))
                .map(|s| s.to_string());
            create_backup(deps, label, out)
        }
    }
}

fn create_backup(
    deps: &CliDependencies,
    label: Option<String>,
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some(rt) = deps.services.db_runtime() else {
        MessageBox::error("DB Runtime not available")
            .message("Embedded database runtime is not enabled")
            .render(out)?;
        return Ok(CommandOutcome::Continue);
    };

    // Get output sink for async task
    let Some(sink) = deps.output() else {
        MessageBox::error("Output not available")
            .message("Cannot stream output")
            .render(out)?;
        return Ok(CommandOutcome::Continue);
    };

    // Return async task
    let rt_clone = Arc::clone(&rt);
    let audit_ctx = build_backup_audit_ctx(deps, label.as_deref());
    let future = async move { execute_backup(sink, rt_clone, label, audit_ctx).await };

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

async fn execute_backup(
    sink: Arc<dyn CommandOutput>,
    rt: Arc<DbRuntimeSupervisor>,
    label: Option<String>,
    audit_ctx: Option<BackupAuditContext>,
) -> io::Result<CommandOutcome> {
    let mut out = StreamedWriter::new(sink);

    writeln!(out)?;
    writeln!(out, "  \x1b[38;5;81m▸\x1b[0m Creating database backup...")?;
    writeln!(out)?;

    // Create progress channel
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<BackupProgress>();

    // Spawn the backup task
    let backup_task = {
        let progress_tx = progress_tx.clone();
        tokio::spawn(async move {
            let callback = move |progress: BackupProgress| {
                let _ = progress_tx.send(progress);
            };
            rt.backup_with_progress(label, Some(callback)).await
        })
    };

    // Run select loop for live progress
    let mut last_step = 0u8;
    let mut backup_task = backup_task;

    let result = loop {
        tokio::select! {
            biased;
            Some(progress) = progress_rx.recv() => {
                match progress {
                    BackupProgress::Preparing if last_step < 1 => {
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Preparing backup...")?;
                        last_step = 1;
                    }
                    BackupProgress::CreatingBackup if last_step < 2 => {
                        if last_step >= 1 {
                            write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Preparing backup       \n")?;
                        }
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Creating backup data...")?;
                        last_step = 2;
                    }
                    BackupProgress::BackupCreated if last_step < 3 => {
                        write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Creating backup data   \n")?;
                        last_step = 3;
                    }
                    BackupProgress::SavingState if last_step < 4 => {
                        write!(out, "    \x1b[38;5;250m◦\x1b[0m    Saving state snapshot...")?;
                        last_step = 4;
                    }
                    BackupProgress::StateSaved if last_step < 5 => {
                        write!(out, "\r    \x1b[38;5;114m✓\x1b[0m    Saving state snapshot   \n")?;
                        last_step = 5;
                    }
                    BackupProgress::Complete => {
                        // Done
                    }
                    _ => {}
                }
            }
            result = &mut backup_task => {
                break result;
            }
        }
    };

    match result {
        Ok(Ok(artifact)) => {
            writeln!(out)?;

            let mut status = StatusBox::new("Backup Complete").field_styled(
                "Status",
                format!("{} Success", SYM_SUCCESS),
                FieldStyle::Success,
            );

            // Get file size if possible
            let size_bytes = std::fs::metadata(&artifact.artifact_path)
                .ok()
                .map(|metadata| {
                    if metadata.is_dir() {
                        calculate_dir_size(Path::new(&artifact.artifact_path))
                    } else {
                        metadata.len()
                    }
                });
            if let Some(size) = size_bytes {
                status = status.field("Size", format_size(size));
            }

            if artifact.state_snapshot_path.is_some() {
                status = status.field("State", "saved");
            }

            // Put full path in its own section
            status = status.section().field("Path", &artifact.artifact_path);

            status.render(&mut out)?;

            if let Some(ctx) = audit_ctx.as_ref() {
                let mut metadata = ctx.base_metadata.clone();
                metadata = metadata.insert("path", artifact.artifact_path.clone());
                if let Some(size) = size_bytes {
                    metadata = metadata.insert("size_bytes", size.to_string());
                }
                if let Some(state) = &artifact.state_snapshot_path {
                    metadata = metadata.insert("state_snapshot", state.clone());
                }
                record_backup_audit(
                    &ctx.services,
                    ctx.actor.clone(),
                    AuditOutcome::Success,
                    metadata,
                );
            }
        }
        Ok(Err(err)) => {
            writeln!(out)?;
            writeln!(out, "    \x1b[38;5;203m✗\x1b[0m    Operation failed")?;
            writeln!(out)?;

            MessageBox::error("Backup Failed")
                .message(err.to_string())
                .render(&mut out)?;

            if let Some(ctx) = audit_ctx.as_ref() {
                let metadata = ctx.base_metadata.clone().insert("error", err.to_string());
                record_backup_audit(
                    &ctx.services,
                    ctx.actor.clone(),
                    AuditOutcome::Failure,
                    metadata,
                );
            }
        }
        Err(err) => {
            writeln!(out)?;
            MessageBox::error("Backup Failed")
                .message(format!("Task error: {}", err))
                .render(&mut out)?;

            if let Some(ctx) = audit_ctx.as_ref() {
                let metadata = ctx
                    .base_metadata
                    .clone()
                    .insert("error", format!("task error: {}", err));
                record_backup_audit(
                    &ctx.services,
                    ctx.actor.clone(),
                    AuditOutcome::Failure,
                    metadata,
                );
            }
        }
    }

    Ok(CommandOutcome::Continue)
}

fn show_backup_list(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<CommandOutcome> {
    let backup_dir = deps.config.runtime.resolve_path("backups");

    if !backup_dir.exists() {
        MessageBox::info("No backups found")
            .message(format!(
                "Backup directory does not exist: {}",
                backup_dir.display()
            ))
            .suggestion("Create a backup with: backup db")
            .render(out)?;
        return Ok(CommandOutcome::Continue);
    }

    let mut backups: Vec<(String, u64, String)> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&backup_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            // Skip metadata files
            if name.ends_with(".json") {
                continue;
            }

            let size = get_size(&path);
            let created = get_created_time(&path);

            backups.push((name, size, created));
        }
    }

    if backups.is_empty() {
        MessageBox::info("No backups found")
            .message("The backup directory is empty")
            .suggestion("Create a backup with: backup db")
            .render(out)?;
        return Ok(CommandOutcome::Continue);
    }

    // Sort by name (which includes timestamp)
    backups.sort_by(|a, b| b.0.cmp(&a.0)); // Newest first

    let mut table = BoxTable::new(vec!["Name".into(), "Size".into(), "Created".into()])
        .with_title(format!("Database Backups ({})", backups.len()));

    for (name, size, created) in backups {
        let size_str = format_size(size);
        table.add_row(vec![name, size_str, created]);
    }

    table.render(out)?;
    writeln!(out)?;
    writeln!(out, "  Backup directory: {}", backup_dir.display())?;

    Ok(CommandOutcome::Continue)
}

fn show_backup_status(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<CommandOutcome> {
    let cfg = &deps.config.db.backup;

    let enabled_status = if cfg.enabled {
        (format!("{} Enabled", SYM_SUCCESS), FieldStyle::Success)
    } else {
        (format!("{} Disabled", SYM_ERROR), FieldStyle::Muted)
    };

    StatusBox::new("Backup Configuration")
        .field_styled("Auto-Backup", &enabled_status.0, enabled_status.1)
        .field("Schedule", &cfg.schedule)
        .field("Retention", format!("{} backups", cfg.retention_count))
        .section()
        .field("Path", &cfg.path)
        .field("Min Disk Space", format!("{} MB", cfg.min_disk_space_mb))
        .field(
            "Require Healthy",
            if cfg.require_healthy { "yes" } else { "no" },
        )
        .field(
            "Verify Integrity",
            if cfg.verify_integrity { "yes" } else { "no" },
        )
        .field(
            "Anomaly Threshold",
            if cfg.anomaly_threshold_pct > 0 {
                format!("{}%", cfg.anomaly_threshold_pct)
            } else {
                "disabled".to_string()
            },
        )
        .render(out)?;

    Ok(CommandOutcome::Continue)
}

fn get_size(path: &Path) -> u64 {
    if path.is_file() {
        std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    } else if path.is_dir() {
        calculate_dir_size(path)
    } else {
        0
    }
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

fn get_created_time(path: &Path) -> String {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.created().ok())
        .map(|t| {
            let datetime: chrono::DateTime<chrono::Local> = t.into();
            datetime.format("%Y-%m-%d %H:%M").to_string()
        })
        .unwrap_or_else(|| "-".to_string())
}

struct BackupAuditContext {
    services: Arc<AppServices>,
    actor: AuditActor,
    base_metadata: AuditMetadata,
}

fn build_backup_audit_ctx(
    deps: &CliDependencies,
    label: Option<&str>,
) -> Option<BackupAuditContext> {
    if !deps.config.audit.enabled {
        return None;
    }
    let actor = cli_actor(deps);
    let host = whoami::fallible::hostname().unwrap_or_else(|_| "unknown-host".to_string());
    let mut metadata = AuditMetadata::default()
        .insert("transport", "cli")
        .insert("command", "backup db")
        .insert("trigger", "cli")
        .insert("host", host);
    if let Some(label) = label {
        metadata = metadata.insert("label", label);
    }
    Some(BackupAuditContext {
        services: Arc::clone(&deps.services),
        actor,
        base_metadata: metadata,
    })
}

fn record_backup_audit(
    services: &Arc<AppServices>,
    actor: AuditActor,
    outcome: AuditOutcome,
    metadata: AuditMetadata,
) {
    match AuditEvent::builder()
        .actor(actor)
        .action("backup::db")
        .target("db-runtime".to_string())
        .outcome(outcome)
        .metadata(metadata)
        .build()
    {
        Ok(event) => {
            if let Err(err) = services.record_audit(event) {
                warn!(error = %err, "cli backup audit append failed");
            }
        }
        Err(err) => warn!(error = %err, "cli backup audit build failed"),
    }
}

fn cli_actor(deps: &CliDependencies) -> AuditActor {
    if let Some(actor) = deps.session_actor() {
        return actor.clone();
    }
    AuditActor::User {
        user_id: format!("cli::{}", whoami::username()),
        role: "operator".to_string(),
    }
}
