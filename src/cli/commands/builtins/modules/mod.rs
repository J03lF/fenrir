use std::io::{self, Write};

use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::services::ServiceKind;
use crate::utils;
use tracing::info;

const DETAILS: &[&str] = &[
    "Zeigt die registrierten Services mit Status",
    "Listet alle eingebauten CLI-Befehle samt Usage",
];

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "modules",
        "Zeigt Modul- und Service-Übersicht",
        "modules",
        DETAILS,
        handle,
    )
}

fn handle(
    deps: &CliDependencies,
    _args: &[&str],
    registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    info!(command = "modules", "modules command invoked");
    writeln!(out, "Fenrir Modulübersicht")?;
    writeln!(out, "======================")?;
    writeln!(out)?;

    render_services(out, deps)?;
    writeln!(out)?;
    render_cli_commands(out, registry)?;

    Ok(CommandOutcome::Continue)
}

fn render_services(out: &mut dyn Write, deps: &CliDependencies) -> io::Result<()> {
    let mut services = deps.services.registry().snapshot();
    writeln!(out, "Aktive Services:")?;
    if services.is_empty() {
        writeln!(out, "  (keine Services registriert)")?;
        return Ok(());
    }

    services.sort_by(|a, b| {
        let kind_cmp = kind_label(a.descriptor.kind).cmp(kind_label(b.descriptor.kind));
        if kind_cmp == std::cmp::Ordering::Equal {
            a.descriptor.name.cmp(b.descriptor.name)
        } else {
            kind_cmp
        }
    });

    let mut table = Table::new(vec![
        "Service".to_string(),
        "Typ".to_string(),
        "Status".to_string(),
        "Seit".to_string(),
        "Hinweis".to_string(),
    ]);

    for snapshot in services {
        let service_name = format!("{} ({})", snapshot.descriptor.name, snapshot.descriptor.id);
        let kind = kind_label(snapshot.descriptor.kind).to_string();
        let status = snapshot.status.label().to_string();
        let since = snapshot
            .since
            .elapsed()
            .ok()
            .map(|duration| utils::format_brief_duration(duration))
            .unwrap_or_else(|| "-".to_string());
        let note = snapshot
            .note
            .filter(|note| !note.is_empty())
            .unwrap_or_else(|| "-".to_string());
        table.add_row(vec![service_name, kind, status, since, note]);
    }

    table.render(out, "  ")
}

fn render_cli_commands(out: &mut dyn Write, registry: &CommandRegistry) -> io::Result<()> {
    writeln!(out, "CLI-Befehle:")?;
    let mut table = Table::new(vec![
        "Befehl".to_string(),
        "Usage".to_string(),
        "Beschreibung".to_string(),
    ]);
    for entry in registry.entries() {
        table.add_row(vec![
            entry.name.clone(),
            entry.usage.to_string(),
            entry.description.clone(),
        ]);
    }
    table.render(out, "  ")
}
fn kind_label(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Infrastructure => "Infrastructure",
        ServiceKind::Transport => "Transport",
        ServiceKind::BackgroundJob => "Background Jobs",
        ServiceKind::Cli => "CLI",
        ServiceKind::Security => "Security",
        ServiceKind::Storage => "Storage",
        ServiceKind::Other => "Other",
    }
}
