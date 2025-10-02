use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::services::scheduler::ScheduledJobSnapshot;
use crate::services::{ServiceControlError, ServiceControlOutcome, ServiceTag};
use crate::utils;
use std::io::{self, Write};

const DETAILS: &[&str] = &[
    "list – zeigt alle registrierten Services mit Status und Hinweis",
    "jobs – listet Scheduler-Jobs mit Intervall",
    "start <id> – startet einen steuerbaren Service",
    "stop <id> [--force] – stoppt einen Service; --force für kritische Dienste",
    "restart <id> [--force] – Neustart eines Service (kritische Dienste benötigen --force)",
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
    let Some(id) = args.first() else {
        writeln!(out, "fehlende Service-ID. Nutzung: services start <id>")?;
        return Ok(());
    };
    match deps.services.start_service(id) {
        Ok(ServiceControlOutcome::Started) => writeln!(out, "Service {id} gestartet.")?,
        Ok(ServiceControlOutcome::AlreadyRunning) => writeln!(out, "Service {id} läuft bereits.")?,
        Ok(other) => writeln!(out, "Service {id}: unerwartetes Ergebnis {other:?}")?,
        Err(err) => render_control_error(err, id, out)?,
    }
    Ok(())
}

fn stop_service(deps: &CliDependencies, args: &[&str], out: &mut dyn Write) -> io::Result<()> {
    let Some(id) = args.first() else {
        writeln!(
            out,
            "fehlende Service-ID. Nutzung: services stop <id> [--force]"
        )?;
        return Ok(());
    };
    let force = args.iter().any(|arg| matches!(*arg, "--force" | "-f"));
    match deps.services.stop_service(id, force) {
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
    let Some(id) = args.first() else {
        writeln!(
            out,
            "fehlende Service-ID. Nutzung: services restart <id> [--force]"
        )?;
        return Ok(());
    };
    let force = args.iter().any(|arg| matches!(*arg, "--force" | "-f"));
    match deps.services.restart_service(id, force) {
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
