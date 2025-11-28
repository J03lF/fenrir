use std::fmt;

pub mod service {
    pub const ALREADY_RUNNING: &str = "scheduler already running, skipping start request";
    pub const STARTING_NOTE: &str = "initialisiere";
    pub const HEARTBEAT_ACTIVE_NOTE: &str = "Heartbeat aktiv";
    pub const HEARTBEAT_OK_NOTE: &str = "Heartbeat OK";
    pub const HEARTBEAT_LOOP_STARTED: &str = "scheduler heartbeat loop started";
    pub const STOPPED_NOTE: &str = "gestoppt";
    pub const HEARTBEAT_STOPPED: &str = "scheduler heartbeat stopped";
    pub const STOP_REQUEST_IGNORED: &str = "scheduler stop requested but heartbeat not running";
    pub const JOBS_INACTIVE_NOTE: &str = "Jobs inaktiv";
    pub const STANDARD_JOBS_ACTIVE_NOTE: &str = "Standard-Jobs aktiv";
    pub const SERVICE_DROPPED: &str = "scheduler service dropped and resources cleaned up";
    pub const DB_AVAILABLE_NOTE: &str = "DB erreichbar";
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
    pub const INVALID_INTERVAL: &str = "intervall muss > 0 sein";

    pub fn job_already_exists(id: impl fmt::Display) -> String {
        format!("job mit id `{id}` existiert bereits")
    }
}

pub mod notes {
    use super::fmt;

    pub fn job_failure(job_id: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("Job {job_id} Fehler: {err}")
    }

    pub fn job_active(id: impl fmt::Display, interval_seconds: impl fmt::Display) -> String {
        format!("Job {id} aktiv ({interval_seconds}s)")
    }

    pub fn job_stopped(id: impl fmt::Display) -> String {
        format!("Job {id} gestoppt")
    }

    pub fn uptime(seconds: impl fmt::Display) -> String {
        format!("Uptime {seconds}s")
    }

    pub fn job_health(failed: usize, degraded: usize) -> String {
        format!("Jobs – failed={failed}, degraded={degraded}")
    }

    pub fn db_unreachable(err: impl fmt::Display) -> String {
        format!("DB nicht erreichbar: {err}")
    }
}

pub mod descriptions {
    pub const TELEMETRY_HEALTH_REFRESH: &str = "Aktualisiert Telemetrie-Uptime und Scheduler-Note";
    pub const SERVICE_HEALTH_SCAN: &str = "Scannt Service-Registry auf Fehlzustände";
    pub const DB_DEFAULT_PING: &str = "Überwacht die Standard-Datenbankverbindung";
}
