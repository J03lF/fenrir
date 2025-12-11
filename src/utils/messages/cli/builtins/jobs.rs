pub mod list {
    pub const NO_JOBS: &str = "No scheduler jobs registered.";
    pub const HEADERS: &[&str] = &["ID", "Interval", "Description", "Status"];
    pub const STATUS_ACTIVE: &str = "active";
    pub const STATUS_INACTIVE: &str = "inactive";
    pub const STATUS_PAUSED: &str = "paused";
}

pub mod status {
    pub const USAGE: &str = "Usage: status job <job-id>";
    pub const HEADER: &str = "Job Detail";
    pub const UNDERLINE: &str = "==========";
    pub const FIELD_ID: &str = "ID";
    pub const FIELD_INTERVAL: &str = "Interval";
    pub const FIELD_DESCRIPTION: &str = "Description";
    pub const FIELD_STATUS: &str = "Status";
    pub const VALUE_ACTIVE: &str = "active";
    pub const VALUE_INACTIVE: &str = "inactive";
    pub const VALUE_PAUSED: &str = "paused";

    pub fn unknown_job(id: &str) -> String {
        format!("job `{id}` not found")
    }
}

pub mod logs {
    pub const USAGE: &str = "Usage: log job <job-id> [--tail N]";
    pub const LOG_UNAVAILABLE: &str = "Application log file is not configured.";
    pub const MISSING_TAIL_VALUE: &str = "--tail expects a number (e.g. --tail 50)";
    pub const TAIL_MINIMUM: &str = "--tail value must be > 0";
    pub const NO_MATCHING_LINES: &str = "No log lines matched the given job id.";

    pub fn invalid_tail(value: &str) -> String {
        format!("invalid --tail value: {value}")
    }

    pub fn unexpected_argument(arg: &str) -> String {
        format!("unexpected argument: {arg}")
    }

    pub fn io_error(err: &std::io::Error) -> String {
        format!("failed to read log file: {err}")
    }

    pub fn log_header(job: &str, path: &std::path::Path, tail: usize) -> String {
        format!(
            "Filtering last {tail} matches for `{job}` in {}",
            path.display()
        )
    }
}

pub mod restart {
    pub const USAGE: &str = "Usage: restart job <job-id>";

    pub fn success(job_id: &str) -> String {
        format!("Job {job_id} restarted.")
    }

    pub fn scheduler_inactive() -> String {
        "Scheduler is not running – restart ignored.".to_string()
    }

    pub fn job_paused(job_id: &str) -> String {
        format!("Job {job_id} is paused – resume it first.")
    }

    pub fn unexpected_outcome(outcome: &str) -> String {
        format!("Restart job failed: unexpected scheduler outcome ({outcome}).")
    }
}

pub mod pause {
    pub const USAGE: &str = "Usage: pause job <job-id>";

    pub fn success(job_id: &str) -> String {
        format!("Job {job_id} paused.")
    }

    pub fn already_paused(job_id: &str) -> String {
        format!("Job {job_id} is already paused.")
    }

    pub fn scheduler_inactive() -> String {
        "Scheduler is not running – pause request deferred.".to_string()
    }

    pub fn unexpected_outcome(outcome: &str) -> String {
        format!("Pause job failed: unexpected scheduler outcome ({outcome}).")
    }
}

pub mod resume {
    pub const USAGE: &str = "Usage: resume job <job-id>";

    pub fn success(job_id: &str) -> String {
        format!("Job {job_id} resumed.")
    }

    pub fn already_active(job_id: &str) -> String {
        format!("Job {job_id} is not paused.")
    }

    pub fn scheduler_inactive() -> String {
        "Scheduler is not running – resume request deferred.".to_string()
    }

    pub fn unexpected_outcome(outcome: &str) -> String {
        format!("Resume job failed: unexpected scheduler outcome ({outcome}).")
    }
}
