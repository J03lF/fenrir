use std::fmt;

pub mod outcome {
    use super::*;

    pub fn db_shell_error(err: impl fmt::Display) -> String {
        format!("db-shell Fehler: {err}")
    }

    pub fn db_shell_failure_note(err: impl fmt::Display) -> String {
        format!("Fehler: {err}")
    }

    pub const DB_SHELL_READY_NOTE: &str = "Bereit für neue Sessions";
}

pub mod runner {
    use super::*;

    pub fn local_session_note(pid: u32) -> String {
        format!("lokale Sitzung pid={pid}")
    }

    pub fn confirmation_failed(err: impl fmt::Display) -> String {
        format!("Bestätigung fehlgeschlagen: {err}")
    }

    pub const CONFIRMATION_RETRY: &str = "Bitte mit 'y' oder 'n' bestätigen.";

    pub fn input_error(err: impl fmt::Display) -> String {
        format!("Eingabefehler: {err}")
    }

    pub fn command_unknown(name: &str) -> String {
        format!("Befehl '{name}' unbekannt – nutze 'help' oder Tab für Vorschläge")
    }

    pub fn command_aborted(name: &str, err: impl fmt::Display) -> String {
        format!("Befehl '{name}' abgebrochen: {err}")
    }

    pub const CLI_STANDBY_NOTE: &str = "Wartet auf nächste Sitzung";
}

pub mod output {
    pub const STATUS_OK: &str = "ok";
    pub const STATUS_EXIT: &str = "exit";
    pub const STATUS_DB: &str = "db";
    pub const STATUS_CONFIRM: &str = "confirm";
    pub const STATUS_ASYNC: &str = "async";
}
