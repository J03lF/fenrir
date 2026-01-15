use crate::audit::{AuditActor, AuditOutcome};
use crate::cli::commands::builtins::jobs::show_job_status;
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::output::{FieldStyle, MessageBox, StatusBox, SYM_ACTIVE, SYM_INACTIVE, SYM_SUCCESS};
use crate::infra::telemetry;
use crate::services::{ServiceIngressProtocol, ServiceMetricSnapshot, ServiceStatus, ServiceTag};
use crate::utils;
use crate::utils::format_offset_datetime;
use crate::utils::messages::cli::builtins::services::list as list_messages;
use crate::utils::messages::cli::builtins::status as status_messages;
use std::io::{self, Write};
use std::time::{Duration, SystemTime};

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
        "fenrir" | "system" | "server" => {
            show_fenrir_status(deps, out)?;
        }
        "db" | "database" => {
            show_db_runtime_status(deps, out)?;
        }
        "service" => {
            let Some(service_id) = tail.first() else {
                MessageBox::warning("Missing service ID")
                    .message("Please specify a service ID")
                    .suggestion("Run 'list services' to see available services")
                    .render(out)?;
                return Ok(CommandOutcome::Continue);
            };
            show_service_status(deps, service_id, out)?;
        }
        "job" => {
            let Some(job_id) = tail.first() else {
                MessageBox::warning("Missing job ID")
                    .message("Please specify a job ID")
                    .suggestion("Run 'list jobs' to see available jobs")
                    .render(out)?;
                return Ok(CommandOutcome::Continue);
            };
            show_job_status(deps, job_id, out)?;
        }
        other => {
            MessageBox::error("Unknown resource")
                .message(format!("Resource '{}' is not recognized", other))
                .suggestion("Valid resources: fenrir, db, service, job")
                .render(out)?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn show_fenrir_status(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let config = &deps.config;

    // ═══════════════════════════════════════════════════════════════════════════
    // Gather data
    // ═══════════════════════════════════════════════════════════════════════════

    // Services
    let services = deps.services.registry().snapshot();
    let active_services = services
        .iter()
        .filter(|s| matches!(s.status, ServiceStatus::Active))
        .count();
    let total_services = services.len();

    // Jobs
    let jobs = deps.services.scheduler_jobs();
    let active_jobs = jobs.iter().filter(|j| j.active && !j.paused).count();
    let total_jobs = jobs.len();

    // DB status
    let db_status = deps.services.db_runtime_status();
    let db_running = db_status.as_ref().map(|s| s.running).unwrap_or(false);
    let db_engine = db_status
        .as_ref()
        .map(|s| s.engine.as_str())
        .unwrap_or("none");

    // Modules count
    let module_count = services
        .iter()
        .filter(|s| s.descriptor.id.starts_with("module:") && !s.descriptor.id.contains("::"))
        .count();

    // Uptime
    let uptime_str = telemetry::uptime()
        .map(utils::format_brief_duration)
        .unwrap_or_else(|| "-".to_string());

    // Telemetry snapshot for performance metrics
    let telemetry_snapshot = telemetry::snapshot();
    let memory_mb = telemetry_snapshot
        .as_ref()
        .and_then(|s| s.metrics.get("process.memory.resident_bytes"))
        .map(|bytes| bytes / (1024 * 1024))
        .unwrap_or(0);
    let cpu_percent = telemetry_snapshot
        .as_ref()
        .and_then(|s| s.metrics.get("process.cpu.usage_percent"))
        .copied()
        .unwrap_or(0);

    // Active connections: SSH connections + security sessions
    let ssh_connections = telemetry_snapshot
        .as_ref()
        .and_then(|s| s.metrics.get("ssh.connections.active"))
        .copied()
        .unwrap_or(0);
    let security_sessions = telemetry_snapshot
        .as_ref()
        .and_then(|s| s.metrics.get("security.sessions.active"))
        .copied()
        .unwrap_or(0);
    let active_sessions = ssh_connections + security_sessions;

    // Audit data for login info
    let audit_events = deps.services.audit_recent(100).unwrap_or_default();
    let now = SystemTime::now();
    let one_day_ago = now
        .checked_sub(Duration::from_secs(24 * 60 * 60))
        .unwrap_or(now);

    // Last login (ssh.login or security.session.issue with User actor)
    let last_login = audit_events.iter().find(|e| {
        (e.action == "ssh.login" || e.action == "security.session.issue")
            && matches!(e.actor, AuditActor::User { .. })
            && matches!(e.outcome, AuditOutcome::Success)
    });
    let last_login_str = last_login
        .map(|e| {
            let user = match &e.actor {
                AuditActor::User { user_id, .. } => user_id.as_str(),
                _ => "unknown",
            };
            let time_ago = e
                .timestamp
                .elapsed()
                .map(utils::format_brief_duration)
                .unwrap_or_else(|_| "?".to_string());
            format!("{} @ {} ago", user, time_ago)
        })
        .unwrap_or_else(|| "-".to_string());

    // Failed logins in last 24h
    let failed_logins_24h = audit_events
        .iter()
        .filter(|e| {
            e.action.contains("denied")
                && matches!(e.outcome, AuditOutcome::Denied)
                && e.timestamp >= one_day_ago
        })
        .count();

    // Last backup
    let last_backup_str = db_status
        .as_ref()
        .and_then(|s| s.last_backup_path.as_ref())
        .map(|path| {
            // Extract timestamp from backup path like postgres-20260109_123456
            if let Some(name) = std::path::Path::new(path).file_name() {
                let name_str = name.to_string_lossy();
                if let Some(pos) = name_str.rfind('-') {
                    let ts_part = &name_str[pos + 1..];
                    if ts_part.len() >= 8 {
                        return format!(
                            "{}-{}-{} {}:{}",
                            &ts_part[0..4],
                            &ts_part[4..6],
                            &ts_part[6..8],
                            ts_part.get(9..11).unwrap_or("00"),
                            ts_part.get(11..13).unwrap_or("00")
                        );
                    }
                }
            }
            "available".to_string()
        })
        .unwrap_or_else(|| "-".to_string());

    // Last migration
    let last_migration_str = db_status
        .as_ref()
        .and_then(|s| s.applied_migrations.last())
        .map(|m| {
            // Extract date from migration name like 20241216_create_...
            if m.len() >= 8 {
                format!("{}-{}-{}", &m[0..4], &m[4..6], &m[6..8])
            } else {
                m.to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string());

    // ═══════════════════════════════════════════════════════════════════════════
    // Build Design 2: Single Box with Sections
    // ═══════════════════════════════════════════════════════════════════════════

    let mut box_builder = StatusBox::new("FENRIR")
        .field("Version", &config.app.version)
        .field("Profile", &config.app.name)
        .field_styled(
            "Status",
            format!("{} Running", SYM_SUCCESS),
            FieldStyle::Success,
        )
        .field("Uptime", &uptime_str);

    // ─── Network ─────────────────────────────────────────────────────────────
    box_builder = box_builder.section_titled("Network");

    let ssh_addr = format!("{}:{}", config.server.ssh.host, config.server.ssh.port);
    box_builder = box_builder.field_styled(
        "SSH",
        format!("{} {}", SYM_ACTIVE, ssh_addr),
        FieldStyle::Success,
    );

    if config.server.enable_http {
        let http_addr = format!("{}:{}", config.server.http.host, config.server.http.port);
        box_builder = box_builder.field_styled(
            "HTTP",
            format!("{} {}", SYM_ACTIVE, http_addr),
            FieldStyle::Success,
        );
    } else {
        box_builder = box_builder.field_styled(
            "HTTP",
            format!("{} disabled", SYM_INACTIVE),
            FieldStyle::Muted,
        );
    }

    if config.server.enable_grpc {
        if let Some(grpc) = &config.server.grpc {
            let grpc_addr = format!("{}:{}", grpc.host, grpc.port);
            box_builder = box_builder.field_styled(
                "gRPC",
                format!("{} {}", SYM_ACTIVE, grpc_addr),
                FieldStyle::Success,
            );
        }
    } else {
        box_builder = box_builder.field_styled(
            "gRPC",
            format!("{} disabled", SYM_INACTIVE),
            FieldStyle::Muted,
        );
    }

    // ─── Database ────────────────────────────────────────────────────────────
    box_builder = box_builder.section_titled("Database");

    let db_status_text = if db_running {
        format!("{} Running", SYM_ACTIVE)
    } else {
        format!("{} Stopped", SYM_INACTIVE)
    };
    let db_style = if db_running {
        FieldStyle::Success
    } else {
        FieldStyle::Error
    };
    box_builder = box_builder
        .field("Engine", db_engine)
        .field_styled("Status", &db_status_text, db_style);

    // Connection mode
    let db_mode = if config.db.runtime.embedded.security.prefer_unix_socket {
        "Unix Socket"
    } else {
        "TCP"
    };
    box_builder = box_builder.field("Mode", db_mode);

    // Backup status
    let backup_text = if last_backup_str != "-" {
        format!("{} {}", SYM_SUCCESS, last_backup_str)
    } else {
        "-".to_string()
    };
    let backup_style = if last_backup_str != "-" {
        FieldStyle::Success
    } else {
        FieldStyle::Muted
    };
    box_builder = box_builder.field_styled("Backup", &backup_text, backup_style);

    // ─── Components ──────────────────────────────────────────────────────────
    box_builder = box_builder.section_titled("Components");

    let services_text = format!("{}/{} {}", active_services, total_services, SYM_ACTIVE);
    let services_style = if active_services == total_services && total_services > 0 {
        FieldStyle::Success
    } else if active_services > 0 {
        FieldStyle::Warning
    } else {
        FieldStyle::Muted
    };
    box_builder = box_builder.field_styled("Services", &services_text, services_style);

    let jobs_text = format!("{}/{} {}", active_jobs, total_jobs, SYM_ACTIVE);
    box_builder = box_builder.field("Jobs", &jobs_text);
    box_builder = box_builder.field("Modules", module_count);

    // ─── Performance ─────────────────────────────────────────────────────────
    box_builder = box_builder.section_titled("Performance");

    box_builder = box_builder.field("Memory", format!("{} MB", memory_mb));
    box_builder = box_builder.field("CPU", format!("{}%", cpu_percent));

    // Request metrics from diagnostics if available
    let http_diag = deps.services.service_diagnostics("http-server");
    let requests_str = http_diag
        .as_ref()
        .and_then(|d| d.latency_p50_ms)
        .map(|_| {
            // We don't have request count, but we have latency
            "active"
        })
        .unwrap_or("-");
    box_builder = box_builder.field("Requests", requests_str);

    let error_rate = http_diag
        .and_then(|d| d.error_rate_pct)
        .map(|r| format!("{:.1}%", r))
        .unwrap_or_else(|| "-".to_string());
    box_builder = box_builder.field("Errors", &error_rate);

    // ─── Security ────────────────────────────────────────────────────────────
    box_builder = box_builder.section_titled("Security");

    box_builder = box_builder.field("Sessions", format!("{} active", active_sessions));

    let failed_style = if failed_logins_24h == 0 {
        FieldStyle::Success
    } else if failed_logins_24h < 5 {
        FieldStyle::Warning
    } else {
        FieldStyle::Error
    };
    box_builder =
        box_builder.field_styled("Failed", format!("{} (24h)", failed_logins_24h), failed_style);

    let identity_provider = config.security.identity.provider.as_str();
    box_builder = box_builder.field("Identity", identity_provider);

    let auth_method = config.db.runtime.embedded.security.auth_method.as_str();
    box_builder = box_builder.field("Auth", auth_method);

    if config.audit.enabled {
        box_builder = box_builder.field_styled(
            "Audit",
            format!("{} enabled", SYM_ACTIVE),
            FieldStyle::Success,
        );
    } else {
        box_builder = box_builder.field_styled(
            "Audit",
            format!("{} disabled", SYM_INACTIVE),
            FieldStyle::Muted,
        );
    }

    // ─── Activity ────────────────────────────────────────────────────────────
    box_builder = box_builder.section_titled("Activity");

    box_builder = box_builder.field("Last Login", &last_login_str);
    box_builder = box_builder.field("Last Backup", &last_backup_str);
    box_builder = box_builder.field("Last Migration", &last_migration_str);

    box_builder.render(out)
}

fn show_db_runtime_status(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let Some(status) = deps.services.db_runtime_status() else {
        MessageBox::info("DB Runtime not available")
            .message("Embedded database runtime is not enabled")
            .suggestion("Check if db.runtime.mode is set to 'embedded' in config")
            .render(out)?;
        return Ok(());
    };

    let running_status = if status.running {
        format!("{} Running", SYM_ACTIVE)
    } else {
        format!("{} Stopped", SYM_INACTIVE)
    };

    let running_style = if status.running {
        FieldStyle::Success
    } else {
        FieldStyle::Error
    };

    let mut box_builder = StatusBox::new("Database Runtime")
        .field("Engine", status.engine.as_str())
        .field_styled("Status", &running_status, running_style)
        .field("Adapter", status.adapter_status.as_str());

    // Add optional fields
    if let Some(port) = status.port {
        box_builder = box_builder.field("Port", port);
    }
    if let Some(pid) = status.pid {
        box_builder = box_builder.field("PID", pid);
    }
    if let Some(uri) = &status.connector_uri {
        box_builder = box_builder.field("Connector", uri);
    }

    // Health section
    box_builder = box_builder.section();

    if let Some(health_time) = status.last_health {
        box_builder = box_builder.field_styled(
            "Last Health",
            format_offset_datetime(health_time),
            FieldStyle::Success,
        );
    }

    if let Some(checkpoint) = status.last_checkpoint {
        box_builder = box_builder.field("Checkpoint", format_offset_datetime(checkpoint));
    }
    if let Some(updated) = status.snapshot_updated_at {
        box_builder = box_builder.field("Snapshot", format_offset_datetime(updated));
    }

    // Migrations section
    if !status.applied_migrations.is_empty() {
        box_builder = box_builder.section();
        box_builder = box_builder.field("Migrations", status.applied_migrations.len());
        if let Some(last) = status.applied_migrations.last() {
            box_builder = box_builder.field("Latest", last);
        }
    }

    // Backup section
    if status.last_backup_path.is_some() || status.last_backup_state_path.is_some() {
        box_builder = box_builder.section();
        if let Some(path) = &status.last_backup_path {
            box_builder = box_builder.field("Backup", path);
        }
        if let Some(state) = &status.last_backup_state_path {
            box_builder = box_builder.field("Backup State", state);
        }
    }

    box_builder.render(out)
}

fn show_service_status(
    deps: &CliDependencies,
    service_id: &str,
    out: &mut dyn Write,
) -> io::Result<()> {
    let Some(snapshot) = deps.services.registry().get(service_id) else {
        MessageBox::error("Service not found")
            .message(format!("Service '{}' does not exist", service_id))
            .suggestion("Run 'list services' to see available services")
            .render(out)?;
        return Ok(());
    };

    let diagnostics = deps.services.service_diagnostics(service_id);

    // Format status with symbol
    let status_text = format_status_with_symbol(snapshot.status);
    let status_style = status_to_style(snapshot.status);

    // Format uptime
    let uptime = snapshot
        .since
        .elapsed()
        .ok()
        .map(utils::format_brief_duration)
        .unwrap_or_else(|| "-".to_string());

    // Format health
    let health_label = render_health_label(snapshot.status, diagnostics.as_ref());
    let health_style = health_to_style(health_label);

    let mut box_builder = StatusBox::new(&snapshot.descriptor.id)
        .field("Name", &snapshot.descriptor.name)
        .field("Kind", snapshot.descriptor.kind.as_str())
        .field_styled("Status", &status_text, status_style)
        .field("Uptime", &uptime);

    // Tags if present
    let tags = render_tags(&snapshot.descriptor.tags);
    if tags != "-" {
        box_builder = box_builder.field("Tags", &tags);
    }

    // Health section
    box_builder = box_builder.section();
    box_builder = box_builder.field_styled("Health", health_label, health_style);
    box_builder = box_builder.field("Heartbeat", format_heartbeat(diagnostics.as_ref()));

    // Metrics
    let p50 = format_latency_ms(diagnostics.and_then(|d| d.latency_p50_ms));
    let p95 = format_latency_ms(diagnostics.and_then(|d| d.latency_p95_ms));
    let error_rate = format_error_rate_pct(diagnostics.and_then(|d| d.error_rate_pct));

    box_builder = box_builder
        .field("Latency P50", &p50)
        .field("Latency P95", &p95)
        .field("Error Rate", &error_rate);

    // Ingress section
    if let Some(ingress) = snapshot.descriptor.ingress.as_ref() {
        box_builder = box_builder.section();

        let protocols = ingress
            .protocols
            .iter()
            .copied()
            .map(ServiceIngressProtocol::as_str)
            .collect::<Vec<_>>()
            .join(", ");

        box_builder = box_builder
            .field("Protocols", &protocols)
            .field("Access", ingress.access.as_str());

        if let Some(route) = ingress.route_prefix.as_ref() {
            box_builder = box_builder.field("Route", route);
        }
    }

    // Note if present
    if let Some(note) = snapshot.note.as_ref() {
        box_builder = box_builder.section();
        box_builder = box_builder.field_styled("Note", note, FieldStyle::Muted);
    }

    box_builder.render(out)
}

// ═══════════════════════════════════════════════════════════════════════════
// Helper functions
// ═══════════════════════════════════════════════════════════════════════════

fn format_status_with_symbol(status: ServiceStatus) -> String {
    let symbol = match status {
        ServiceStatus::Active => SYM_ACTIVE,
        ServiceStatus::Starting => "◐",
        ServiceStatus::Stopped | ServiceStatus::Standby => SYM_INACTIVE,
        ServiceStatus::Failed | ServiceStatus::Degraded => "◉",
    };
    format!("{} {}", symbol, status.label())
}

fn status_to_style(status: ServiceStatus) -> FieldStyle {
    match status {
        ServiceStatus::Active => FieldStyle::Success,
        ServiceStatus::Starting => FieldStyle::Warning,
        ServiceStatus::Stopped | ServiceStatus::Standby => FieldStyle::Muted,
        ServiceStatus::Failed | ServiceStatus::Degraded => FieldStyle::Error,
    }
}

fn health_to_style(health: &str) -> FieldStyle {
    if health.contains("healthy") || health.contains("✓") {
        FieldStyle::Success
    } else if health.contains("degraded") || health.contains("stale") {
        FieldStyle::Warning
    } else if health.contains("unhealthy") || health.contains("✗") {
        FieldStyle::Error
    } else {
        FieldStyle::Muted
    }
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

fn format_latency_ms(value: Option<f64>) -> String {
    value
        .map(|latency| format!("{:.0} ms", latency))
        .unwrap_or_else(|| "-".to_string())
}

fn format_error_rate_pct(value: Option<f64>) -> String {
    value
        .map(|rate| format!("{:.1}%", rate))
        .unwrap_or_else(|| "-".to_string())
}
