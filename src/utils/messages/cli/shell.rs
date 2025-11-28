use std::fmt;

pub mod outcome {
    use super::*;

    pub fn db_shell_error(err: impl fmt::Display) -> String {
        format!("db-shell error: {err}")
    }

    pub fn db_shell_failure_note(err: impl fmt::Display) -> String {
        format!("error: {err}")
    }

    pub const DB_SHELL_READY_NOTE: &str = "ready for new sessions";
}

pub mod runner {
    use super::*;

    pub fn local_session_note(pid: u32) -> String {
        format!("local session pid={pid}")
    }

    pub fn confirmation_failed(err: impl fmt::Display) -> String {
        format!("confirmation failed: {err}")
    }

    pub const CONFIRMATION_RETRY: &str = "Please confirm with 'y' or 'n'.";

    pub fn input_error(err: impl fmt::Display) -> String {
        format!("input error: {err}")
    }

    pub fn command_unknown(name: &str) -> String {
        format!("command '{name}' is unknown – use 'help' or tab for suggestions")
    }

    pub fn command_aborted(name: &str, err: impl fmt::Display) -> String {
        format!("command '{name}' aborted: {err}")
    }

    pub const CLI_STANDBY_NOTE: &str = "waiting for next session";
}

pub mod output {
    pub const STATUS_OK: &str = "ok";
    pub const STATUS_EXIT: &str = "exit";
    pub const STATUS_DB: &str = "db";
    pub const STATUS_CONFIRM: &str = "confirm";
    pub const STATUS_ASYNC: &str = "async";
}
