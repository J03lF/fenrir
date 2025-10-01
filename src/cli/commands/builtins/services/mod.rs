use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::utils;
use std::io::{self, Write};

const DETAILS: &[&str] = &["list – zeigt alle registrierten Services mit Status und Hinweis"];

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "services",
        "Zeigt den Status registrierter Applikationsservices",
        "services [list]",
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
            svc.status.label().to_string(),
            since,
            svc.descriptor.description.to_string(),
            note,
        ]);
    }

    table.render(out, "  ")
}
