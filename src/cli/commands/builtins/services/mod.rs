use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::services::scheduler::ScheduledJobSnapshot;
use crate::services::{
    ServiceActionReport, ServiceControlError, ServiceControlOutcome, ServiceTag,
};
use crate::utils;
use std::io::{self, Write};
use tracing::warn;

const TRANSPORT: &str = "cli";

const DETAILS: &[&str] = &[
    "list – zeigt alle registrierten Services mit Status und Hinweis",
    "jobs – listet Scheduler-Jobs mit Intervall",
    "start <id> – startet einen steuerbaren Service",
    "stop <id|--all> [--force] – stoppt Service oder alle nicht-core Services",
    "restart <id|--all> [--force] – Neustart Service oder aller nicht-core Services",
];

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "services",
        "Zeigt den Status registrierter Applikationsservices",
        "services [list|jobs|start|stop|restart]",
        DETAILS,
        handle,
    )
}

fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let (action, rest) = if let Some((first, rest)) = args.split_first() {
        (*first, rest)
    } else {
        ("list", &[][..])
    };

    match action {
        "list" => list_services(deps, out)?,
        "jobs" => list_jobs(deps, out)?,
        "start" => start_service(deps, rest, out)?,
        "stop" => stop_service(deps, rest, out)?,
        "restart" => restart_service(deps, rest, out)?,
        other => {
            writeln!(out, "unbekannte Aktion: {other}")?;
            writeln!(out, "verfügbar: services [list|jobs|start|stop|restart]")?;
        }
    }
    Ok(CommandOutcome::Continue)
}

fn list_services(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let mut entries = deps.services.registry().snapshot();
    if entries.is_empty() {
        writeln!(out, "Keine Services registriert.")?;
        return Ok(());
    }

    entries.sort_by(|a, b| a.descriptor.id.cmp(b.descriptor.id));

    let mut table = Table::new(vec![
        "ID".to_string(),
        "Name".to_string(),
        "Typ".to_string(),
        "Tags".to_string(),
        "Status".to_string(),
        "Seit".to_string(),
        "Beschreibung".to_string(),
        "Hinweis".to_string(),
    ]);

    for svc in entries {
        let since = svc
            .since
            .elapsed()
            .ok()
            .map(|duration| utils::format_brief_duration(duration))
            .unwrap_or_else(|| "-".to_string());
        let note = svc
            .note
            .filter(|note| !note.is_empty())
            .unwrap_or_else(|| "-".to_string());
        table.add_row(vec![
            svc.descriptor.id.to_string(),
            svc.descriptor.name.to_string(),
            svc.descriptor.kind.as_str().to_string(),
            render_tags(&svc.descriptor.tags),
            svc.status.label().to_string(),
            since,
            svc.descriptor.description.to_string(),
            note,
        ]);
    }

    table.render(out, "  ")
}

fn render_tags(tags: &[ServiceTag]) -> String {
    if tags.is_empty() {
        return "-".to_string();
    }
    let labels: Vec<&'static str> = tags.iter().map(ServiceTag::as_str).collect();
    labels.join(", ")
}

fn list_jobs(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let jobs = deps.services.scheduler_service().jobs();
    if jobs.is_empty() {
        writeln!(out, "Keine Scheduler-Jobs registriert.")?;
        return Ok(());
    }

    let mut table = Table::new(vec![
        "ID".to_string(),
        "Intervall".to_string(),
        "Beschreibung".to_string(),
        "Status".to_string(),
    ]);

    for job in jobs {
        render_job(&mut table, job);
    }

    table.render(out, "  ")
}

fn render_job(table: &mut Table, job: ScheduledJobSnapshot) {
    table.add_row(vec![
        job.id,
        format!("{}s", job.interval.as_secs()),
        job.description,
        if job.active {
            "active".to_string()
        } else {
            "inactive".to_string()
        },
    ]);
}

fn start_service(deps: &CliDependencies, args: &[&str], out: &mut dyn Write) -> io::Result<()> {
    if args
        .iter()
        .any(|arg| matches!(*arg, "--all" | "-a" | "all"))
    {
        let reports = deps.services.start_all_non_core();
        render_bulk_results(deps, out, "start", false, reports)?;
        return Ok(());
    }
    let Some(id) = args.first() else {
        writeln!(out, "fehlende Service-ID. Nutzung: services start <id>")?;
        return Ok(());
    };
    let result = deps.services.start_service(id);
    record_cli_service_action(deps, "start", id, false, &result);
    match result {
        Ok(ServiceControlOutcome::Started) => writeln!(out, "Service {id} gestartet.")?,
        Ok(ServiceControlOutcome::AlreadyRunning) => writeln!(out, "Service {id} läuft bereits.")?,
        Ok(other) => writeln!(out, "Service {id}: unerwartetes Ergebnis {other:?}")?,
        Err(err) => render_control_error(err, id, out)?,
    }
    Ok(())
}

fn stop_service(deps: &CliDependencies, args: &[&str], out: &mut dyn Write) -> io::Result<()> {
    let all = args
        .iter()
        .any(|arg| matches!(*arg, "--all" | "-a" | "all"));
    let Some(id) = args.first() else {
        if all {
            let force = args.iter().any(|arg| matches!(*arg, "--force" | "-f"));
            let reports = deps.services.stop_all_non_core(force);
            render_bulk_results(deps, out, "stop", force, reports)?;
        } else {
            writeln!(
                out,
                "fehlende Service-ID. Nutzung: services stop <id|--all> [--force]"
            )?;
        }
        return Ok(());
    };
    if matches!(*id, "--all" | "-a" | "all") {
        let force = args.iter().any(|arg| matches!(*arg, "--force" | "-f"));
        let reports = deps.services.stop_all_non_core(force);
        render_bulk_results(deps, out, "stop", force, reports)?;
        return Ok(());
    }
    let force = args.iter().any(|arg| matches!(*arg, "--force" | "-f"));
    let result = deps.services.stop_service(id, force);
    record_cli_service_action(deps, "stop", id, force, &result);
    match result {
        Ok(ServiceControlOutcome::Stopped) => writeln!(out, "Service {id} gestoppt.")?,
        Ok(ServiceControlOutcome::AlreadyStopped) => {
            writeln!(out, "Service {id} war bereits gestoppt.")?
        }
        Ok(other) => writeln!(out, "Service {id}: unerwartetes Ergebnis {other:?}")?,
        Err(err) => render_control_error(err, id, out)?,
    }
    Ok(())
}

fn restart_service(deps: &CliDependencies, args: &[&str], out: &mut dyn Write) -> io::Result<()> {
    let all = args
        .iter()
        .any(|arg| matches!(*arg, "--all" | "-a" | "all"));
    let Some(id) = args.first() else {
        if all {
            let force = args.iter().any(|arg| matches!(*arg, "--force" | "-f"));
            let reports = deps.services.restart_all_non_core(force);
            render_bulk_results(deps, out, "restart", force, reports)?;
        } else {
            writeln!(
                out,
                "fehlende Service-ID. Nutzung: services restart <id|--all> [--force]"
            )?;
        }
        return Ok(());
    };
    if matches!(*id, "--all" | "-a" | "all") {
        let force = args.iter().any(|arg| matches!(*arg, "--force" | "-f"));
        let reports = deps.services.restart_all_non_core(force);
        render_bulk_results(deps, out, "restart", force, reports)?;
        return Ok(());
    }
    let force = args.iter().any(|arg| matches!(*arg, "--force" | "-f"));
    let result = deps.services.restart_service(id, force);
    record_cli_service_action(deps, "restart", id, force, &result);
    match result {
        Ok(ServiceControlOutcome::Restarted) => writeln!(out, "Service {id} neu gestartet.")?,
        Ok(ServiceControlOutcome::Started) => writeln!(out, "Service {id} neu gestartet.")?,
        Ok(ServiceControlOutcome::AlreadyRunning) => writeln!(out, "Service {id} läuft bereits.")?,
        Ok(other) => writeln!(out, "Service {id}: unerwartetes Ergebnis {other:?}")?,
        Err(err) => render_control_error(err, id, out)?,
    }
    Ok(())
}

fn render_control_error(
    err: ServiceControlError,
    requested_id: &str,
    out: &mut dyn Write,
) -> io::Result<()> {
    match err {
        ServiceControlError::UnknownService(_) => {
            writeln!(out, "Unbekannter Service: {requested_id}")?
        }
        ServiceControlError::NotControllable(_) => writeln!(
            out,
            "Service {requested_id} unterstützt keine Steuerung über diese CLI."
        )?,
        ServiceControlError::ForceRequired(_) => writeln!(
            out,
            "Service {requested_id} ist als kritisch markiert. --force erforderlich."
        )?,
        ServiceControlError::CoreLocked(_) => writeln!(
            out,
            "Service {requested_id} gehört zur core-Plattform und kann nicht gestoppt oder neu gestartet werden."
        )?,
        ServiceControlError::OperationFailed { source, .. } => {
            writeln!(out, "Operation fehlgeschlagen: {source}")?
        }
    }
    Ok(())
}

fn render_bulk_results(
    deps: &CliDependencies,
    out: &mut dyn Write,
    action: &str,
    force: bool,
    reports: Vec<ServiceActionReport>,
) -> io::Result<()> {
    if reports.is_empty() {
        writeln!(out, "Keine steuerbaren Services gefunden.")?;
        return Ok(());
    }
    writeln!(out, "Ergebnisse für services {action} --all:")?;
    let mut success = 0usize;
    let mut failures = Vec::new();
    for report in reports {
        match report.result {
            Ok(outcome) => {
                success += 1;
                writeln!(out, "  - {}: {}", report.id, outcome.as_str())?
            }
            Err(err) => {
                failures.push((report.id.clone(), err.to_string()));
                writeln!(out, "  - {}: Fehler ({err})", report.id)?;
            }
        }
    }
    record_cli_bulk_action(deps, action, force, success, failures.len(), failures);
    Ok(())
}

fn record_cli_service_action(
    deps: &CliDependencies,
    action: &str,
    target: &str,
    force: bool,
    result: &Result<ServiceControlOutcome, ServiceControlError>,
) {
    let actor = cli_actor();
    let mut metadata = base_metadata(action, force);
    let outcome = match result {
        Ok(control) => {
            metadata = metadata.insert("service_outcome", control.as_str());
            AuditOutcome::Success
        }
        Err(err) => {
            metadata = metadata
                .insert("error_code", control_error_code(err))
                .insert("message", control_error_message(err));
            AuditOutcome::Failure
        }
    };
    push_audit_event(deps, actor, action, target, outcome, metadata);
}

fn record_cli_bulk_action(
    deps: &CliDependencies,
    action: &str,
    force: bool,
    success: usize,
    failure_count: usize,
    failures: Vec<(String, String)>,
) {
    let actor = cli_actor();
    let mut metadata = base_metadata(action, force)
        .insert("mode", "bulk")
        .insert("success", success.to_string())
        .insert("failed", failure_count.to_string());
    if failure_count > 0 {
        let ids: Vec<String> = failures.iter().map(|(id, _)| id.clone()).collect();
        let messages: Vec<String> = failures.iter().map(|(_, msg)| msg.clone()).collect();
        metadata = metadata
            .insert("failed_ids", ids.join(","))
            .insert("errors", messages.join(" | "));
    }
    let outcome = if failure_count == 0 {
        AuditOutcome::Success
    } else {
        AuditOutcome::Failure
    };
    push_audit_event(deps, actor, action, "--all", outcome, metadata);
}

fn cli_actor() -> AuditActor {
    AuditActor::User {
        user_id: format!("cli::{}", whoami::username()),
        role: "operator".to_string(),
    }
}

fn base_metadata(action: &str, force: bool) -> AuditMetadata {
    let host = whoami::fallible::hostname().unwrap_or_else(|_| "unknown-host".to_string());
    AuditMetadata::default()
        .insert("transport", TRANSPORT)
        .insert("command", format!("services {action}"))
        .insert("force", if force { "true" } else { "false" })
        .insert("host", host)
}

fn push_audit_event(
    deps: &CliDependencies,
    actor: AuditActor,
    action: &str,
    target: &str,
    outcome: AuditOutcome,
    metadata: AuditMetadata,
) {
    match AuditEvent::builder()
        .actor(actor)
        .action(format!("service::{action}"))
        .target(target.to_string())
        .outcome(outcome)
        .metadata(metadata)
        .build()
    {
        Ok(event) => {
            if let Err(err) = deps.services.record_audit(event) {
                warn!(error = %err, "cli audit append failed");
            }
        }
        Err(err) => warn!(error = %err, "cli audit build failed"),
    }
}

fn control_error_code(err: &ServiceControlError) -> &'static str {
    match err {
        ServiceControlError::UnknownService(_) => "unknown_service",
        ServiceControlError::NotControllable(_) => "not_controllable",
        ServiceControlError::ForceRequired(_) => "force_required",
        ServiceControlError::CoreLocked(_) => "core_locked",
        ServiceControlError::OperationFailed { .. } => "operation_failed",
    }
}

fn control_error_message(err: &ServiceControlError) -> String {
    match err {
        ServiceControlError::UnknownService(_) => "Service ist nicht registriert".to_string(),
        ServiceControlError::NotControllable(_) => {
            "Service erlaubt keine Steuerung über CLI".to_string()
        }
        ServiceControlError::ForceRequired(_) => {
            "Aktion erfordert --force für kritischen Service".to_string()
        }
        ServiceControlError::CoreLocked(_) => "Core-Service blockiert für Stop/Restart".to_string(),
        ServiceControlError::OperationFailed { source, .. } => source.to_string(),
    }
}
