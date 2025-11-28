use std::fmt;

pub mod service {
    pub const ALREADY_RUNNING: &str = "scheduler already running, skipping start request";
    pub const STARTING_NOTE: &str = "initializing";
    pub const HEARTBEAT_ACTIVE_NOTE: &str = "heartbeat active";
    pub const HEARTBEAT_OK_NOTE: &str = "Heartbeat OK";
    pub const HEARTBEAT_LOOP_STARTED: &str = "scheduler heartbeat loop started";
    pub const STOPPED_NOTE: &str = "stopped";
    pub const HEARTBEAT_STOPPED: &str = "scheduler heartbeat stopped";
    pub const STOP_REQUEST_IGNORED: &str = "scheduler stop requested but heartbeat not running";
    pub const JOBS_INACTIVE_NOTE: &str = "jobs inactive";
    pub const STANDARD_JOBS_ACTIVE_NOTE: &str = "standard jobs active";
    pub const SERVICE_DROPPED: &str = "scheduler service dropped and resources cleaned up";
    pub const DB_AVAILABLE_NOTE: &str = "db reachable";
    pub const DB_PING_FAILED: &str = "db ping failed";
}

pub mod debug {
    pub const HEARTBEAT_SENT: &str = "heartbeat sent";
    pub const JOB_CLEARED: &str = "scheduler job cleared";
}

pub mod errors {
    use super::fmt;

    pub const JOB_FAILED: &str = "scheduler job failed";
    pub const SCHEDULER_NOT_RUNNING: &str = "scheduler not running";
    pub const INVALID_INTERVAL: &str = "interval must be > 0";

    pub fn job_already_exists(id: impl fmt::Display) -> String {
        format!("job with id `{id}` already exists")
    }
}

pub mod notes {
    use super::fmt;

    pub fn job_failure(job_id: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("job {job_id} error: {err}")
    }

    pub fn job_active(id: impl fmt::Display, interval_seconds: impl fmt::Display) -> String {
        format!("job {id} active ({interval_seconds}s)")
    }

    pub fn job_stopped(id: impl fmt::Display) -> String {
        format!("job {id} stopped")
    }

    pub fn uptime(seconds: impl fmt::Display) -> String {
        format!("Uptime {seconds}s")
    }

    pub fn job_health(failed: usize, degraded: usize) -> String {
        format!("Jobs – failed={failed}, degraded={degraded}")
    }

    pub fn db_unreachable(err: impl fmt::Display) -> String {
        format!("db unreachable: {err}")
    }
}

pub mod descriptions {
    pub const TELEMETRY_HEALTH_REFRESH: &str =
        "Refreshes telemetry uptime and scheduler note";
    pub const SERVICE_HEALTH_SCAN: &str = "Scans the service registry for fault states";
    pub const DB_DEFAULT_PING: &str = "Monitors the default database connection";
}
