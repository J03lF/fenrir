use std::error::Error as StdError;
use std::fmt;
use std::time::Duration;

use crate::utils::messages::services::scheduler::errors as scheduler_errors;

#[derive(Debug)]
pub enum SchedulerError {
    SchedulerNotStarted,
    JobAlreadyExists(String),
    JobNotFound(String),
    InvalidInterval,
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchedulerError::SchedulerNotStarted => {
                write!(f, "{}", scheduler_errors::SCHEDULER_NOT_RUNNING)
            }
            SchedulerError::JobAlreadyExists(id) => {
                write!(f, "{}", scheduler_errors::job_already_exists(id))
            }
            SchedulerError::JobNotFound(id) => {
                write!(f, "{}", scheduler_errors::job_not_found(id))
            }
            SchedulerError::InvalidInterval => {
                write!(f, "{}", scheduler_errors::INVALID_INTERVAL)
            }
        }
    }
}

impl StdError for SchedulerError {}

#[derive(Clone, Debug)]
pub struct ScheduledJobSpec {
    pub id: String,
    pub interval: Duration,
    pub initial_delay: Option<Duration>,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct ScheduledJobSnapshot {
    pub id: String,
    pub interval: Duration,
    pub description: String,
    pub active: bool,
    pub paused: bool,
}

pub fn job_metrics_id(job_id: &str) -> String {
    format!("job:{job_id}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobControlOutcome {
    Restarted,
    SchedulerInactive,
    Paused,
    AlreadyPaused,
    Resumed,
    AlreadyActive,
}

impl JobControlOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobControlOutcome::Restarted => "restarted",
            JobControlOutcome::SchedulerInactive => "scheduler_inactive",
            JobControlOutcome::Paused => "paused",
            JobControlOutcome::AlreadyPaused => "already_paused",
            JobControlOutcome::Resumed => "resumed",
            JobControlOutcome::AlreadyActive => "already_active",
        }
    }
}
