use std::fmt;

pub mod errors {
    use super::fmt;

    pub fn unknown_service(id: impl fmt::Display) -> String {
        format!("service `{id}` is unknown")
    }

    pub fn not_controllable(id: impl fmt::Display) -> String {
        format!("service `{id}` does not support runtime control")
    }

    pub fn force_required(id: impl fmt::Display) -> String {
        format!("service `{id}` is marked as critical – --force required")
    }

    pub fn core_locked(id: impl fmt::Display) -> String {
        format!("service `{id}` is marked as core and cannot be stopped")
    }

    pub fn operation_failed(id: impl fmt::Display, source: impl fmt::Display) -> String {
        format!("operation for service `{id}` failed: {source}")
    }
}

pub mod outcomes {
    pub const STARTED: &str = "started";
    pub const ALREADY_RUNNING: &str = "already_running";
    pub const STOPPED: &str = "stopped";
    pub const ALREADY_STOPPED: &str = "already_stopped";
    pub const RESTARTED: &str = "restarted";
}
