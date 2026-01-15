use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionContext, ShellEnvironment,
};
use crate::services::{AppServices, ServiceControlError, ServiceControlOutcome};
use std::io::Write;
use tokio::runtime::Handle;

pub fn command() -> CommandEntry {
    const SUBS: &[crate::cli::commands::registry::CommandSubcommand] = &[
        crate::cli::commands::registry::CommandSubcommand::new(
            "status",
            &[],
            &[],
            "Show db runtime status",
        ),
        crate::cli::commands::registry::CommandSubcommand::new(
            "logs",
            &[],
            &[CommandArgument::optional("tail")],
            "Tail db runtime logs",
        ),
        crate::cli::commands::registry::CommandSubcommand::new(
            "start",
            &[],
            &[],
            "Start db runtime (embedded mode only)",
        ),
        crate::cli::commands::registry::CommandSubcommand::new(
            "stop",
            &[],
            &[CommandArgument::optional("force")],
            "Stop db runtime",
        ),
        crate::cli::commands::registry::CommandSubcommand::new(
            "restart",
            &[],
            &[CommandArgument::optional("force")],
            "Restart db runtime",
        ),
        crate::cli::commands::registry::CommandSubcommand::new(
            "backup",
            &[],
            &[CommandArgument::optional("label")],
            "Create db runtime backup",
        ),
        crate::cli::commands::registry::CommandSubcommand::new(
            "restore",
            &[],
            &[CommandArgument::required("file")],
            "Restore db runtime from backup",
        ),
    ];
    let shape = CommandShape::new("db runtime", &[], &[], SUBS);
    CommandEntry::with_shape(
        "db runtime",
        "Database runtime control",
        "db runtime <subcommand>",
        &["Manage embedded db runtime"],
        handle,
        shape,
    )
}

pub fn handle(
    deps: &CliDependencies,
    tokens: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> std::io::Result<CommandOutcome> {
    let services: &AppServices = deps.services.as_ref();
    let sub = tokens.first().copied().unwrap_or("status");
    let args = &tokens[1..];
    match sub {
        "status" => {
            let tail = args
                .first()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            write_status(out, services, tail)?;
        }
        "logs" => {
            let tail = args
                .first()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(50);
            for line in services.db_runtime_logs(tail) {
                writeln!(out, "{line}")?;
            }
        }
        "start" => write_control(out, services.start_service("db-runtime"))?,
        "stop" => {
            let force = args.first().map(|v| *v == "--force").unwrap_or(false);
            write_control(out, services.stop_service("db-runtime", force))?
        }
        "restart" => {
            let force = args.first().map(|v| *v == "--force").unwrap_or(false);
            write_control(out, services.restart_service("db-runtime", force))?
        }
        "backup" => {
            let label = args.first().map(|s| s.to_string());
            match services.db_runtime() {
                Some(rt) => match block_on_any(rt.backup(label)) {
                    Ok(artifact) => {
                        if let Some(state_path) = artifact.state_snapshot_path {
                            writeln!(
                                out,
                                "backup created at {} (state: {state_path})",
                                artifact.artifact_path
                            )?
                        } else {
                            writeln!(out, "backup created at {}", artifact.artifact_path)?
                        }
                    }
                    Err(err) => writeln!(out, "error: {err}")?,
                },
                None => writeln!(out, "db-runtime not available")?,
            }
        }
        "restore" => {
            if args.is_empty() {
                writeln!(out, "usage: db runtime restore <file>")?;
            } else if let Some(rt) = services.db_runtime() {
                match block_on_any(rt.restore(args[0])) {
                    Ok(()) => writeln!(out, "restore completed")?,
                    Err(err) => writeln!(out, "error: {err}")?,
                }
            } else {
                writeln!(out, "db-runtime not available")?;
            }
        }
        other => {
            writeln!(out, "unknown subcommand {other}")?;
        }
    }
    Ok(CommandOutcome::Continue)
}

pub fn completion(ctx: &CompletionContext<'_>) -> Vec<String> {
    let subs = [
        "status", "logs", "start", "stop", "restart", "backup", "restore",
    ];
    match ctx.active_index {
        0 => subs
            .iter()
            .filter(|s| s.starts_with(ctx.prefix))
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn write_status(out: &mut dyn Write, services: &AppServices, tail: usize) -> std::io::Result<()> {
    if let Some(status) = services.db_runtime_status() {
        writeln!(out, "engine: {}", status.engine.as_str())?;
        writeln!(out, "running: {}", status.running)?;
        if let Some(uri) = status.connector_uri {
            writeln!(out, "uri: {uri}")?;
        }
        if let Some(port) = status.port {
            writeln!(out, "port: {port}")?;
        }
        if let Some(pid) = status.pid {
            writeln!(out, "pid: {pid}")?;
        }
        if let Some(h) = status.last_health {
            writeln!(out, "last_health: {h}")?;
        }
        if tail > 0 {
            writeln!(out, "logs (tail {}):", tail)?;
            for line in services.db_runtime_logs(tail) {
                writeln!(out, "  {line}")?;
            }
        }
    } else {
        writeln!(out, "db-runtime not available")?;
    }
    Ok(())
}

fn write_control(
    out: &mut dyn Write,
    result: Result<ServiceControlOutcome, ServiceControlError>,
) -> std::io::Result<()> {
    match result {
        Ok(outcome) => writeln!(out, "{}", outcome.as_str())?,
        Err(err) => writeln!(out, "error: {err}")?,
    }
    Ok(())
}

fn block_on_any<F, T>(fut: F) -> Result<T, anyhow::Error>
where
    F: std::future::Future<Output = Result<T, anyhow::Error>>,
{
    if let Ok(handle) = Handle::try_current() {
        handle.block_on(fut)
    } else {
        tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .block_on(fut)
    }
}
