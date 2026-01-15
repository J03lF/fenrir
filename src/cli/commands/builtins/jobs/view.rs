use std::io::{self, Write};

use crate::cli::commands::registry::CliDependencies;
use crate::cli::output::{BoxTable, FieldStyle, MessageBox, StatusBox, SYM_ACTIVE, SYM_INACTIVE};
use crate::services::scheduler::ScheduledJobSnapshot;
use crate::services::JobLogSnapshot;
use crate::utils::messages::cli::builtins::jobs as job_messages;

pub(crate) fn render_jobs_table(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let jobs = deps.services.scheduler_jobs();

    if jobs.is_empty() {
        MessageBox::info("No jobs")
            .message("No scheduled jobs are currently registered")
            .render(out)?;
        return Ok(());
    }

    let mut table = BoxTable::new(vec![
        "ID".to_string(),
        "INTERVAL".to_string(),
        "DESCRIPTION".to_string(),
        "STATUS".to_string(),
    ])
    .with_title("Scheduled Jobs")
    .with_count();

    for job in jobs {
        let status_display = format_job_status_display(&job);
        table.add_row(vec![
            job.id.clone(),
            format!("{}s", job.interval.as_secs()),
            job.description.clone(),
            status_display,
        ]);
    }

    table.render(out)
}

pub(super) fn render_job_status(job: &ScheduledJobSnapshot, out: &mut dyn Write) -> io::Result<()> {
    let status_text = format_job_status_display(job);
    let status_style = job_status_style(job);

    StatusBox::new(&job.id)
        .field("Interval", format!("{}s", job.interval.as_secs()))
        .field("Description", &job.description)
        .field_styled("Status", &status_text, status_style)
        .render(out)
}

pub(super) fn render_job_logs(
    snapshot: JobLogSnapshot,
    out: &mut dyn Write,
    tail: usize,
) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        job_messages::logs::log_header(&snapshot.job_id, &snapshot.source, tail)
    )?;

    if snapshot.lines.is_empty() {
        MessageBox::info("No log entries")
            .message(format!(
                "No matching log entries for job '{}'",
                snapshot.job_id
            ))
            .render(out)?;
        return Ok(());
    }

    for line in snapshot.lines {
        writeln!(out, "{}", line)?;
    }
    Ok(())
}

fn format_job_status_display(job: &ScheduledJobSnapshot) -> String {
    if job.paused {
        format!("◫ {}", job_messages::list::STATUS_PAUSED)
    } else if job.active {
        format!("{} {}", SYM_ACTIVE, job_messages::list::STATUS_ACTIVE)
    } else {
        format!("{} {}", SYM_INACTIVE, job_messages::list::STATUS_INACTIVE)
    }
}

fn job_status_style(job: &ScheduledJobSnapshot) -> FieldStyle {
    if job.paused {
        FieldStyle::Warning
    } else if job.active {
        FieldStyle::Success
    } else {
        FieldStyle::Muted
    }
}

pub(crate) fn job_status_label(job: &ScheduledJobSnapshot) -> &'static str {
    if job.paused {
        job_messages::list::STATUS_PAUSED
    } else if job.active {
        job_messages::list::STATUS_ACTIVE
    } else {
        job_messages::list::STATUS_INACTIVE
    }
}
