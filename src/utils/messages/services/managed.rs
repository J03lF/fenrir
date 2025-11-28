use std::fmt;

pub mod errors {
    use super::fmt;

    pub fn unknown_service(id: impl fmt::Display) -> String {
        format!("service `{id}` ist unbekannt")
    }

    pub fn not_controllable(id: impl fmt::Display) -> String {
        format!("service `{id}` unterstützt keine Laufzeitsteuerung")
    }

    pub fn force_required(id: impl fmt::Display) -> String {
        format!("service `{id}` ist als kritisch markiert – --force erforderlich")
    }

    pub fn core_locked(id: impl fmt::Display) -> String {
        format!("service `{id}` ist als core markiert und kann nicht gestoppt werden")
    }

    pub fn operation_failed(id: impl fmt::Display, source: impl fmt::Display) -> String {
        format!("operation für service `{id}` fehlgeschlagen: {source}")
    }
}

pub mod outcomes {
    pub const STARTED: &str = "started";
    pub const ALREADY_RUNNING: &str = "already_running";
    pub const STOPPED: &str = "stopped";
    pub const ALREADY_STOPPED: &str = "already_stopped";
    pub const RESTARTED: &str = "restarted";
}
