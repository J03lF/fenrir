use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::services::ServiceStatus;
use std::fmt;
use std::io::{self, Write};
use std::time::{Duration, SystemTime};

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "services",
        "Zeigt den Status registrierter Applikationsservices",
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
    let action = args.first().copied().unwrap_or("list");
    match action {
        "list" => list_services(deps, out)?,
        other => {
            writeln!(out, "unbekannte Aktion: {other}")?;
            writeln!(out, "verfügbar: services [list]")?;
        }
    }
    Ok(CommandOutcome::Continue)
}

fn list_services(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let entries = deps.services.registry().snapshot();
    if entries.is_empty() {
        writeln!(out, "Keine Services registriert.")?;
        return Ok(());
    }

    let mut id_width = 2usize;
    let mut name_width = 4usize;
    let mut kind_width = 4usize;
    for svc in &entries {
        id_width = id_width.max(svc.descriptor.id.len());
        name_width = name_width.max(svc.descriptor.name.len());
        kind_width = kind_width.max(svc.descriptor.kind.as_str().len());
    }

    writeln!(out, "Services:")?;
    let header = format!(
        "  {:<width_id$}  {:<width_name$}  {:<width_kind$}  {:<10}  {:<10}  {}",
        "ID",
        "Name",
        "Typ",
        "Status",
        "Seit",
        "Beschreibung",
        width_id = id_width,
        width_name = name_width,
        width_kind = kind_width,
    );
    let separator = format!(
        "  {id}  {name}  {kind}  {status:-<10}  {since:-<10}  -",
        id = "-".repeat(id_width),
        name = "-".repeat(name_width),
        kind = "-".repeat(kind_width),
        status = "",
        since = "",
    );
    writeln!(out, "{header}")?;
    writeln!(out, "{separator}")?;

    for svc in entries {
        let line = format!(
            "  {:<width_id$}  {:<width_name$}  {:<width_kind$}  {:<10}  {:<10}  {}",
            svc.descriptor.id,
            svc.descriptor.name,
            svc.descriptor.kind.as_str(),
            ServiceStatusDisplay(svc.status),
            SinceDisplay::new(svc.since),
            svc.descriptor.description,
            width_id = id_width,
            width_name = name_width,
            width_kind = kind_width,
        );
        writeln!(out, "{line}")?;
        if let Some(note) = svc.note.as_deref() {
            writeln!(out, "    ↳ {note}")?;
        }
    }
    Ok(())
}

struct ServiceStatusDisplay(ServiceStatus);

impl fmt::Display for ServiceStatusDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.label())
    }
}

struct SinceDisplay(Option<Duration>);

impl SinceDisplay {
    fn new(since: SystemTime) -> Self {
        Self(since.elapsed().ok())
    }
}

impl fmt::Display for SinceDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(duration) => {
                let secs = duration.as_secs();
                if secs >= 3600 {
                    write!(f, "{}h", secs / 3600)
                } else if secs >= 60 {
                    write!(f, "{}m", secs / 60)
                } else {
                    write!(f, "{}s", secs)
                }
            }
            None => f.write_str("-"),
        }
    }
}
