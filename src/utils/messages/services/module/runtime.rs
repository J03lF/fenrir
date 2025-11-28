use std::fmt;

pub mod logs {
    pub const LIST_FOR_AUTOSTART_FAILED: &str = "failed to list modules for auto-start";
    pub const AUTOSTART_FAILED: &str = "auto-start failed";
    pub const STOP_DURING_SYNC_FAILED: &str = "failed to stop module during sync";
    pub const STOP_BEFORE_UPDATE_FAILED: &str = "failed to stop module before update";
    pub const MODULE_STARTED: &str = "module started successfully";
    pub const MODULE_STOPPED: &str = "module stopped successfully";
    pub const RESTARTING_MODULE: &str = "restarting module";
}

pub mod notes {
    use super::fmt;

    pub fn running_with_pid(pid: impl fmt::Display) -> String {
        format!("läuft (PID {pid})")
    }

    pub const RUNNING: &str = "läuft";
    pub const STOPPED: &str = "gestoppt";
}
