use super::modules;
use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionContext, CompletionKind, ShellEnvironment,
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

enum ServiceCliAction {
    Start,
    Stop,
    Restart,
}

impl ServiceCliAction {
    fn verb(&self) -> &'static str {
        match self {
            ServiceCliAction::Start => "start",
            ServiceCliAction::Stop => "stop",
            ServiceCliAction::Restart => "restart",
        }
    }
}

const SERVICE_RESOURCE_OPTIONS: &[&str] = &["service", "services", "module", "modules"];
const LIST_RESOURCE_OPTIONS: &[&str] = &["services", "jobs", "modules"];
const SERVICE_TARGET_GLOBAL_OPTIONS: &[&str] = &["--all", "-a", "all"];
const SERVICE_FORCE_OPTIONS: &[&str] = &["--force", "-f"];

const START_DETAILS: &[&str] = &[
    "start service <id|--all> – startet einen steuerbaren Service",
    "start module <name> – startet ein installiertes Modul",
];

const STOP_DETAILS: &[&str] = &[
    "stop service <id|--all> [--force] – stoppt einen Service",
    "stop module <name> – stoppt ein Modul",
];

const RESTART_DETAILS: &[&str] = &[
    "restart service <id|--all> [--force] – Neustart von Services",
    "restart module <name> – startet ein Modul neu",
];

const LIST_DETAILS: &[&str] = &[
    "list services – zeigt registrierte Services",
    "list jobs – listet Scheduler-Jobs",
    "list modules – zeigt installierte Module (mit Runtime)",
];

const SERVICE_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(SERVICE_RESOURCE_OPTIONS));
const SERVICE_TARGET_ARGUMENT: CommandArgument = CommandArgument::required("target")
    .with_completion(CompletionKind::Dynamic(complete_action_targets));
const SERVICE_FORCE_ARGUMENT: CommandArgument = CommandArgument::optional("flag")
    .with_completion(CompletionKind::Dynamic(complete_force_flags))
    .variadic();

const START_ARGUMENTS: &[CommandArgument] = &[SERVICE_RESOURCE_ARGUMENT, SERVICE_TARGET_ARGUMENT];
const START_SHAPE: CommandShape = CommandShape::new("start", &[], START_ARGUMENTS, &[]);

const STOP_ARGUMENTS: &[CommandArgument] = &[
    SERVICE_RESOURCE_ARGUMENT,
    SERVICE_TARGET_ARGUMENT,
    SERVICE_FORCE_ARGUMENT,
];
const STOP_SHAPE: CommandShape = CommandShape::new("stop", &[], STOP_ARGUMENTS, &[]);

const RESTART_ARGUMENTS: &[CommandArgument] = &[
    SERVICE_RESOURCE_ARGUMENT,
    SERVICE_TARGET_ARGUMENT,
    SERVICE_FORCE_ARGUMENT,
];
const RESTART_SHAPE: CommandShape = CommandShape::new("restart", &[], RESTART_ARGUMENTS, &[]);

const LIST_ARGUMENTS: &[CommandArgument] = &[CommandArgument::optional("resource")
    .with_completion(CompletionKind::Static(LIST_RESOURCE_OPTIONS))];
const LIST_SHAPE: CommandShape = CommandShape::new("list", &[], LIST_ARGUMENTS, &[]);

pub fn start_command() -> CommandEntry {
    CommandEntry::with_shape(
        "start",
        "Startet Services oder Module",
        "start <service|module> <ziel>",
        START_DETAILS,
        handle_start,
        START_SHAPE,
    )
}

pub fn stop_command() -> CommandEntry {
    CommandEntry::with_shape(
        "stop",
        "Stoppt Services oder Module kontrolliert",
        "stop <service|module> <ziel> [--force]",
        STOP_DETAILS,
        handle_stop,
        STOP_SHAPE,
    )
}

pub fn restart_command() -> CommandEntry {
    CommandEntry::with_shape(
        "restart",
        "Startet Services oder Module neu",
        "restart <service|module> <ziel> [--force]",
        RESTART_DETAILS,
        handle_restart,
        RESTART_SHAPE,
    )
}

pub fn list_command() -> CommandEntry {
    CommandEntry::with_shape(
        "list",
        "Listet Ressourcen (Services, Jobs, Module)",
        "list <services|jobs|modules>",
        LIST_DETAILS,
        handle_list,
        LIST_SHAPE,
    )
}

fn handle_start(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    route_service_action(ServiceCliAction::Start, deps, args, out)?;
    Ok(CommandOutcome::Continue)
}

fn handle_stop(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    route_service_action(ServiceCliAction::Stop, deps, args, out)?;
    Ok(CommandOutcome::Continue)
}

fn handle_restart(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    route_service_action(ServiceCliAction::Restart, deps, args, out)?;
    Ok(CommandOutcome::Continue)
}

fn handle_list(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let (resource, rest) = match args.split_first() {
        Some((value, tail)) => (*value, tail),
        None => ("services", &[][..]),
    };

    match resource.to_ascii_lowercase().as_str() {
        "services" | "service" => {
            if !rest.is_empty() {
                writeln!(
                    out,
                    "Hinweis: 'list services' erwartet keine weiteren Argumente."
                )?;
            }
            list_services(deps, out)?;
        }
        "jobs" | "job" => {
            if !rest.is_empty() {
                writeln!(
                    out,
                    "Hinweis: 'list jobs' erwartet keine weiteren Argumente."
                )?;
            }
            list_jobs(deps, out)?;
        }
        "modules" | "module" => {
            modules::run_module_command(deps, "list", rest, out)?;
        }
        other => {
            writeln!(out, "unbekannte Ressource: {other}")?;
            writeln!(out, "verfügbar: list services|jobs|modules")?;
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

fn service_id_suggestions(deps: &CliDependencies) -> Vec<String> {
    let mut ids: Vec<String> = deps
        .services
        .registry()
        .snapshot()
        .into_iter()
        .map(|svc| svc.descriptor.id.to_string())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn complete_action_targets(deps: &CliDependencies, ctx: &CompletionContext<'_>) -> Vec<String> {
    let Some(command) = ctx.tokens.first().copied() else {
        return Vec::new();
    };

    if !matches!(command, "start" | "stop" | "restart") {
        return Vec::new();
    }

    let Some(resource) = ctx.tokens.get(1).copied() else {
        return Vec::new();
    };

    if matches!(resource, "service" | "services") {
        let mut suggestions: Vec<String> = SERVICE_TARGET_GLOBAL_OPTIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect();

        for id in service_id_suggestions(deps) {
            if !suggestions.iter().any(|candidate| candidate == &id) {
                suggestions.push(id);
            }
        }

        return suggestions;
    }

    if matches!(resource, "module" | "modules") {
        return modules::complete_module_ids(deps, ctx);
    }

    Vec::new()
}

fn complete_force_flags(_deps: &CliDependencies, ctx: &CompletionContext<'_>) -> Vec<String> {
    let Some(resource) = ctx.tokens.get(1).copied() else {
        return Vec::new();
    };

    if matches!(resource, "service" | "services") {
        return SERVICE_FORCE_OPTIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect();
    }

    Vec::new()
}

fn route_service_action(
    action: ServiceCliAction,
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    if args.is_empty() {
        render_action_usage(out, action)?;
        return Ok(());
    }

    match args[0].to_ascii_lowercase().as_str() {
        "service" | "services" => {
            let tail = if args.len() > 1 { &args[1..] } else { &[] };
            match action {
                ServiceCliAction::Start => start_service(deps, tail, out),
                ServiceCliAction::Stop => stop_service(deps, tail, out),
                ServiceCliAction::Restart => restart_service(deps, tail, out),
            }
        }
        "module" | "modules" => {
            let tail = if args.len() > 1 { &args[1..] } else { &[] };
            if tail.is_empty() {
                render_action_usage(out, action)
            } else {
                modules::run_module_command(deps, action.verb(), tail, out)
            }
        }
        other => {
            writeln!(out, "unbekannte Ressource: {other}")?;
            writeln!(out, "gültig: service | module")
        }
    }
}

fn render_action_usage(out: &mut dyn Write, action: ServiceCliAction) -> io::Result<()> {
    match action {
        ServiceCliAction::Start => writeln!(
            out,
            "Nutzung: start service <id|--all> | start module <name>"
        )?,
        ServiceCliAction::Stop => writeln!(
            out,
            "Nutzung: stop service <id|--all> [--force] | stop module <name>"
        )?,
        ServiceCliAction::Restart => writeln!(
            out,
            "Nutzung: restart service <id|--all> [--force] | restart module <name>"
        )?,
    }
    Ok(())
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
        writeln!(
            out,
            "fehlende Service-ID. Nutzung: start service <id|--all>"
        )?;
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
                "fehlende Service-ID. Nutzung: stop service <id|--all> [--force]"
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
                "fehlende Service-ID. Nutzung: restart service <id|--all> [--force]"
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
    writeln!(out, "Ergebnisse für {action} service --all:")?;
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
        .insert("command", format!("{action} service"))
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
