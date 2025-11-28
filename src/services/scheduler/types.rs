use std::error::Error as StdError;
use std::fmt;
use std::time::Duration;

use crate::utils::messages::services::scheduler::errors as scheduler_errors;

#[derive(Debug)]
pub enum SchedulerError {
    SchedulerNotStarted,
    JobAlreadyExists(String),
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
}
