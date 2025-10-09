use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CommandSubcommand, CompletionContext, CompletionKind, ShellEnvironment,
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
const COLOR_DIM: &str = "\x1b[38;5;244m";
const COLOR_ACCENT: &str = "\x1b[38;5;214m";
const COLOR_RESET: &str = "\x1b[0m";

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

const DETAILS: &[&str] = &[
    "list – zeigt alle registrierten Services mit Status und Hinweis",
    "jobs – listet Scheduler-Jobs mit Intervall",
    "start <id> – startet einen steuerbaren Service (veraltet, nutze 'start service <id>')",
    "stop <id|--all> [--force] – veraltet, nutze 'stop service <id|--all>'",
    "restart <id|--all> [--force] – veraltet, nutze 'restart service <id|--all>'",
];

const START_DETAILS: &[&str] = &[
    "start service <id> – startet einen steuerbaren Service",
    "start service --all – startet alle nicht-core Services",
];

const STOP_DETAILS: &[&str] = &[
    "stop service <id> [--force] – stoppt einen Service",
    "stop service --all [--force] – stoppt alle nicht-core Services",
];

const RESTART_DETAILS: &[&str] = &[
    "restart service <id> [--force] – startet Service neu",
    "restart service --all [--force] – Neustart aller nicht-core Services",
];

const LIST_DETAILS: &[&str] = &[
    "list services – zeigt registrierte Services",
    "list jobs – listet Scheduler-Jobs",
    "list modules – zeigt verfügbare Module (in Vorbereitung)",
];

const SERVICES_ALIASES: &[&str] = &["service", "svc"];
const SERVICE_RESOURCE_OPTIONS: &[&str] = &["service", "services", "module", "modules"];
const SERVICE_TARGET_GLOBAL_OPTIONS: &[&str] = &["--all", "-a", "all"];
const SERVICE_FORCE_OPTIONS: &[&str] = &["--force", "-f"];
const LIST_RESOURCE_OPTIONS: &[&str] = &["services", "jobs", "modules"];

const SERVICE_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(SERVICE_RESOURCE_OPTIONS));
const SERVICE_TARGET_ARGUMENT: CommandArgument = CommandArgument::required("target")
    .with_completion(CompletionKind::Dynamic(complete_service_targets));
const SERVICE_FORCE_ARGUMENT: CommandArgument = CommandArgument::optional("flag")
    .with_completion(CompletionKind::Static(SERVICE_FORCE_OPTIONS))
    .variadic();
const SERVICE_LIST_ARGUMENT: CommandArgument = CommandArgument::optional("resource")
    .with_completion(CompletionKind::Static(LIST_RESOURCE_OPTIONS));

const SERVICES_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new(
        "list",
        &[],
        &[SERVICE_LIST_ARGUMENT],
        "Services oder Jobs anzeigen",
    ),
    CommandSubcommand::new("jobs", &[], &[], "Scheduler-Jobs anzeigen"),
    CommandSubcommand::new(
        "start",
        &[],
        &[SERVICE_TARGET_ARGUMENT],
        "Service starten (Legacy-Pfad)",
    ),
    CommandSubcommand::new(
        "stop",
        &[],
        &[SERVICE_TARGET_ARGUMENT, SERVICE_FORCE_ARGUMENT],
        "Service stoppen (Legacy-Pfad)",
    ),
    CommandSubcommand::new(
        "restart",
        &[],
        &[SERVICE_TARGET_ARGUMENT, SERVICE_FORCE_ARGUMENT],
        "Service neu starten (Legacy-Pfad)",
    ),
];

const SERVICES_SHAPE: CommandShape =
    CommandShape::new("services", SERVICES_ALIASES, &[], SERVICES_SUBCOMMANDS);

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

const LIST_ARGUMENTS: &[CommandArgument] = &[SERVICE_LIST_ARGUMENT];
const LIST_SHAPE: CommandShape = CommandShape::new("list", &[], LIST_ARGUMENTS, &[]);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "services",
        "Zeigt den Status registrierter Applikationsservices",
        "services [list|jobs|start|stop|restart]",
        DETAILS,
        handle,
        SERVICES_SHAPE,
    )
}

pub fn start_command() -> CommandEntry {
    CommandEntry::with_shape(
        "start",
        "Startet Ressourcen wie Services",
        "start service <id|--all>",
        START_DETAILS,
        handle_start,
        START_SHAPE,
    )
}

pub fn stop_command() -> CommandEntry {
    CommandEntry::with_shape(
        "stop",
        "Stoppt Ressourcen kontrolliert",
        "stop service <id|--all> [--force]",
        STOP_DETAILS,
        handle_stop,
        STOP_SHAPE,
    )
}

pub fn restart_command() -> CommandEntry {
    CommandEntry::with_shape(
        "restart",
        "Startet Ressourcen neu",
        "restart service <id|--all> [--force]",
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
        "start" => {
            warn_deprecated(out, "services start", "start service <id>")?;
            start_service(deps, rest, out)?;
        }
        "stop" => {
            warn_deprecated(out, "services stop", "stop service <id>")?;
            stop_service(deps, rest, out)?;
        }
        "restart" => {
            warn_deprecated(out, "services restart", "restart service <id>")?;
            restart_service(deps, rest, out)?;
        }
        other => {
            writeln!(out, "unbekannte Aktion: {other}")?;
            writeln!(out, "verfügbar: services [list|jobs|start|stop|restart]")?;
        }
    }
    Ok(CommandOutcome::Continue)
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
    if args.is_empty() {
        list_services(deps, out)?;
        return Ok(CommandOutcome::Continue);
    }
    match args[0] {
        "services" | "service" => list_services(deps, out)?,
        "jobs" | "job" => list_jobs(deps, out)?,
        "modules" | "module" => module_placeholder(out, "list")?,
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

fn complete_service_targets(deps: &CliDependencies, ctx: &CompletionContext<'_>) -> Vec<String> {
    let Some(command) = ctx.tokens.first().copied() else {
        return Vec::new();
    };

    let mut expects_service_targets = false;

    match command {
        "start" | "stop" | "restart" => {
            if let Some(resource) = ctx.tokens.get(1) {
                if matches!(*resource, "service" | "services") {
                    expects_service_targets = true;
                }
            }
        }
        "services" | "service" => {
            if let Some(action) = ctx.tokens.get(1) {
                if matches!(*action, "start" | "stop" | "restart") {
                    expects_service_targets = true;
                }
            }
        }
        _ => {}
    }

    if !expects_service_targets {
        return Vec::new();
    }

    let mut suggestions: Vec<String> = SERVICE_TARGET_GLOBAL_OPTIONS
        .iter()
        .map(|value| (*value).to_string())
        .collect();

    for id in service_id_suggestions(deps) {
        if !suggestions.iter().any(|candidate| candidate == &id) {
            suggestions.push(id);
        }
    }

    suggestions
}

fn route_service_action(
    action: ServiceCliAction,
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    let usage = match action {
        ServiceCliAction::Start => "start service <id|--all>",
        ServiceCliAction::Stop => "stop service <id|--all> [--force]",
        ServiceCliAction::Restart => "restart service <id|--all> [--force]",
    };

    if args.is_empty() {
        writeln!(out, "Nutzung: {usage}")?;
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
        "module" | "modules" => module_placeholder(out, action.verb()),
        other => {
            writeln!(out, "unbekannte Ressource: {other}")?;
            writeln!(out, "gültig: service | module")
        }
    }
}

fn module_placeholder(out: &mut dyn Write, action: &str) -> io::Result<()> {
    writeln!(
        out,
        "{dim}[Info]{reset} modules {action} steht noch aus – Distribution-Workflow folgt.",
        dim = COLOR_DIM,
        reset = COLOR_RESET,
        action = action
    )
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

fn warn_deprecated(out: &mut dyn Write, legacy: &str, modern: &str) -> io::Result<()> {
    writeln!(
        out,
        "{dim}[Hinweis]{reset} '{legacy}' wird entfernt – nutze {accent}{modern}{reset}.",
        dim = COLOR_DIM,
        accent = COLOR_ACCENT,
        reset = COLOR_RESET,
        legacy = legacy,
        modern = modern
    )
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
