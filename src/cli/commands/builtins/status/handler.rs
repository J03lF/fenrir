use crate::cli::commands::builtins::jobs::show_job_status;
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::services::{ServiceIngressProtocol, ServiceMetricSnapshot, ServiceStatus, ServiceTag};
use crate::utils;
use crate::utils::messages::cli::builtins::services::list as list_messages;
use crate::utils::messages::cli::builtins::status as status_messages;
use std::io::{self, Write};
use std::time::Duration;

pub(super) fn handle_status_command(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "{}", status_messages::USAGE)?;
        return Ok(CommandOutcome::Continue);
    };

    match *resource {
        "db" | "database" => {
            show_db_runtime_status(deps, out)?;
        }
        "service" => {
            let Some(service_id) = tail.first() else {
                writeln!(out, "{}", status_messages::missing_service_id())?;
                return Ok(CommandOutcome::Continue);
            };
            show_service_status(deps, service_id, out)?;
        }
        "job" => {
            let Some(job_id) = tail.first() else {
                writeln!(out, "{}", status_messages::missing_job_id())?;
                return Ok(CommandOutcome::Continue);
            };
            show_job_status(deps, job_id, out)?;
        }
        other => {
            writeln!(out, "{}", status_messages::unknown_resource(other))?;
            writeln!(out, "{}", status_messages::VALID_RESOURCES)?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn show_db_runtime_status(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    if let Some(status) = deps.services.db_runtime_status() {
        writeln!(out, "DB Runtime Status")?;
        writeln!(out, "=================")?;
        writeln!(out, "Engine: {}", status.engine.as_str())?;
        writeln!(out, "Running: {}", if status.running { "yes" } else { "no" })?;
        if let Some(uri) = status.connector_uri {
            writeln!(out, "URI: {uri}")?;
        }
        if let Some(port) = status.port {
            writeln!(out, "Port: {port}")?;
        }
        if let Some(pid) = status.pid {
            writeln!(out, "PID: {pid}")?;
        }
        if let Some(health) = status.last_health {
            writeln!(out, "Last Health: {health}")?;
        }
    } else {
        writeln!(out, "db-runtime not available (mode != embedded?)")?;
    }
    Ok(())
}

fn show_service_status(
    deps: &CliDependencies,
    service_id: &str,
    out: &mut dyn Write,
) -> io::Result<()> {
    let Some(snapshot) = deps.services.registry().get(service_id) else {
        writeln!(out, "{}", status_messages::unknown_service(service_id))?;
        return Ok(());
    };
    let diagnostics = deps.services.service_diagnostics(service_id);

    writeln!(out, "Service Detail")?;
    writeln!(out, "==============")?;
    writeln!(out, "ID: {}", snapshot.descriptor.id)?;
    writeln!(out, "Name: {}", snapshot.descriptor.name)?;
    writeln!(out, "Kind: {}", snapshot.descriptor.kind.as_str())?;
    writeln!(out, "Status: {}", snapshot.status.label())?;
    let since = snapshot
        .since
        .elapsed()
        .ok()
        .map(utils::format_brief_duration)
        .unwrap_or_else(|| "-".to_string());
    writeln!(out, "Seit: {}", since)?;
    writeln!(out, "Tags: {}", render_tags(&snapshot.descriptor.tags))?;
    writeln!(
        out,
        "Health: {}",
        render_health_label(snapshot.status, diagnostics.as_ref())
    )?;
    writeln!(
        out,
        "Letzter Heartbeat: {}",
        format_heartbeat(diagnostics.as_ref())
    )?;
    writeln!(
        out,
        "Latenz P50: {} ms",
        format_latency(diagnostics.and_then(|diag| diag.latency_p50_ms))
    )?;
    writeln!(
        out,
        "Latenz P95: {} ms",
        format_latency(diagnostics.and_then(|diag| diag.latency_p95_ms))
    )?;
    writeln!(
        out,
        "Fehlerquote: {} %",
        format_error_rate(diagnostics.and_then(|diag| diag.error_rate_pct))
    )?;
    if let Some(note) = snapshot.note.as_ref() {
        writeln!(out, "Hinweis: {note}")?;
    }
    if let Some(ingress) = snapshot.descriptor.ingress.as_ref() {
        let protocols = ingress
            .protocols
            .iter()
            .copied()
            .map(ServiceIngressProtocol::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "Ingress Protocols: {protocols}")?;
        writeln!(out, "Access: {}", ingress.access.as_str())?;
        if let Some(route) = ingress.route_prefix.as_ref() {
            writeln!(out, "Route: {route}")?;
        }
    }
    Ok(())
}

fn render_tags(tags: &[ServiceTag]) -> String {
    if tags.is_empty() {
        return "-".to_string();
    }
    let labels: Vec<&'static str> = tags.iter().map(ServiceTag::as_str).collect();
    labels.join(", ")
}

fn render_health_label(
    status: ServiceStatus,
    metrics: Option<&ServiceMetricSnapshot>,
) -> &'static str {
    const STALE_THRESHOLD: Duration = Duration::from_secs(180);
    if let Some(snapshot) = metrics {
        if let Some(last) = snapshot.last_heartbeat_elapsed() {
            if last >= STALE_THRESHOLD {
                return list_messages::HEALTH_STALE;
            }
        }
        if let Some(err) = snapshot.error_rate_pct {
            if err >= 5.0 {
                return list_messages::HEALTH_DEGRADED;
            }
        }
        return list_messages::HEALTH_HEALTHY;
    }
    match status {
        ServiceStatus::Failed | ServiceStatus::Degraded => list_messages::HEALTH_DEGRADED,
        _ => list_messages::HEALTH_UNKNOWN,
    }
}

fn format_heartbeat(metrics: Option<&ServiceMetricSnapshot>) -> String {
    metrics
        .and_then(|snapshot| snapshot.last_heartbeat_elapsed())
        .map(utils::format_brief_duration)
        .unwrap_or_else(|| "-".to_string())
}

fn format_latency(value: Option<f64>) -> String {
    value
        .map(|latency| format!("{latency:.0}"))
        .unwrap_or_else(|| "-".to_string())
}

fn format_error_rate(value: Option<f64>) -> String {
    value
        .map(|rate| format!("{rate:.1}"))
        .unwrap_or_else(|| "-".to_string())
}
