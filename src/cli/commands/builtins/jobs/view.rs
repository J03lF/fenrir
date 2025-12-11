use std::io::{self, Write};

use crate::cli::commands::registry::CliDependencies;
use crate::cli::commands::table::Table;
use crate::services::scheduler::ScheduledJobSnapshot;
use crate::services::JobLogSnapshot;
use crate::utils::messages::cli::builtins::jobs as job_messages;

pub(crate) fn render_jobs_table(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<()> {
    let jobs = deps.services.scheduler_jobs();
    if jobs.is_empty() {
        writeln!(out, "{}", job_messages::list::NO_JOBS)?;
        return Ok(());
    }

    let mut table = Table::new(
        job_messages::list::HEADERS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    );

    for job in jobs {
        table.add_row(render_job_row(job));
    }

    table.render(out, "  ")
}

pub(super) fn render_job_status(job: &ScheduledJobSnapshot, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "{}", job_messages::status::HEADER)?;
    writeln!(out, "{}", job_messages::status::UNDERLINE)?;
    writeln!(
        out,
        "{}: {}",
        job_messages::status::FIELD_ID,
        job.id.as_str()
    )?;
    writeln!(
        out,
        "{}: {}s",
        job_messages::status::FIELD_INTERVAL,
        job.interval.as_secs()
    )?;
    writeln!(
        out,
        "{}: {}",
        job_messages::status::FIELD_DESCRIPTION,
        job.description.as_str()
    )?;
    writeln!(
        out,
        "{}: {}",
        job_messages::status::FIELD_STATUS,
        job_status_label(job)
    )?;
    Ok(())
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
        writeln!(out, "{}", job_messages::logs::NO_MATCHING_LINES)?;
        return Ok(());
    }
    for line in snapshot.lines {
        writeln!(out, "{}", line)?;
    }
    Ok(())
}

fn render_job_row(job: ScheduledJobSnapshot) -> Vec<String> {
    let status = job_status_label(&job).to_string();
    vec![
        job.id,
        format!("{}s", job.interval.as_secs()),
        job.description,
        status,
    ]
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
