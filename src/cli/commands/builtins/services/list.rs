use crate::cli::commands::builtins::jobs::view::render_jobs_table;
use crate::cli::commands::registry::CliDependencies;
use crate::cli::output::{BoxTable, MessageBox, SYM_ACTIVE, SYM_INACTIVE};
use crate::services::ServiceStatus;
use crate::utils;
use std::io::{self, Write};

pub(super) fn list_services(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let mut entries = deps.services.registry().snapshot();
    entries.retain(|svc| !is_module_placeholder(&svc.descriptor.id));

    if entries.is_empty() {
        MessageBox::info("No services")
            .message("No services are currently registered")
            .render(out)?;
        return Ok(());
    }

    entries.sort_by(|a, b| a.descriptor.id.cmp(&b.descriptor.id));

    // Use simplified headers for cleaner display
    let mut table = BoxTable::new(vec![
        "ID".to_string(),
        "NAME".to_string(),
        "STATUS".to_string(),
        "UPTIME".to_string(),
        "NOTE".to_string(),
    ])
    .with_title("Services")
    .with_count();

    for svc in entries {
        let since = svc
            .since
            .elapsed()
            .ok()
            .map(utils::format_brief_duration)
            .unwrap_or_else(|| "-".to_string());

        let note = svc
            .note
            .filter(|note| !note.is_empty())
            .unwrap_or_else(|| "-".to_string());

        // Format status with symbol
        let status_display = format_status_display(svc.status);

        table.add_row(vec![
            svc.descriptor.id.to_string(),
            svc.descriptor.name.to_string(),
            status_display,
            since,
            note,
        ]);
    }

    table.render(out)
}

pub(super) fn list_jobs(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    render_jobs_table(deps, out)
}

fn format_status_display(status: ServiceStatus) -> String {
    let symbol = match status {
        ServiceStatus::Active => SYM_ACTIVE,
        ServiceStatus::Starting => "◐",
        ServiceStatus::Stopped | ServiceStatus::Standby => SYM_INACTIVE,
        ServiceStatus::Failed | ServiceStatus::Degraded => "◉",
    };
    format!("{} {}", symbol, status.label())
}

fn is_module_placeholder(id: &str) -> bool {
    id.starts_with("module:") && !id.contains("::")
}
