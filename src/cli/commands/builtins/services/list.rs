use crate::cli::commands::registry::CliDependencies;
use crate::cli::commands::table::Table;
use crate::services::scheduler::ScheduledJobSnapshot;
use crate::services::ServiceTag;
use crate::utils;
use crate::utils::messages::cli::builtins::services::list as list_messages;
use std::io::{self, Write};

pub(super) fn list_services(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let mut entries = deps.services.registry().snapshot();
    if entries.is_empty() {
        writeln!(out, "{}", list_messages::NO_SERVICES)?;
        return Ok(());
    }

    entries.sort_by(|a, b| a.descriptor.id.cmp(&b.descriptor.id));

    let mut table = Table::new(
        list_messages::SERVICE_HEADERS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    );

    for svc in entries {
        let since = svc
            .since
            .elapsed()
            .ok()
            .map(utils::format_brief_duration)
            .unwrap_or_else(|| list_messages::EMPTY_VALUE.to_string());
        let note = svc
            .note
            .filter(|note| !note.is_empty())
            .unwrap_or_else(|| list_messages::EMPTY_VALUE.to_string());
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

pub(super) fn list_jobs(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let jobs = deps.services.scheduler_service().jobs();
    if jobs.is_empty() {
        writeln!(out, "{}", list_messages::NO_JOBS)?;
        return Ok(());
    }

    let mut table = Table::new(
        list_messages::JOB_HEADERS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    );

    for job in jobs {
        render_job(&mut table, job);
    }

    table.render(out, "  ")
}

fn render_tags(tags: &[ServiceTag]) -> String {
    if tags.is_empty() {
        return list_messages::EMPTY_VALUE.to_string();
    }
    let labels: Vec<&'static str> = tags.iter().map(ServiceTag::as_str).collect();
    labels.join(", ")
}

fn render_job(table: &mut Table, job: ScheduledJobSnapshot) {
    table.add_row(vec![
        job.id,
        format!("{}s", job.interval.as_secs()),
        job.description,
        if job.active {
            list_messages::STATUS_ACTIVE.to_string()
        } else {
            list_messages::STATUS_INACTIVE.to_string()
        },
    ]);
}
