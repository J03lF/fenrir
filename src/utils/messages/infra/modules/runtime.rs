use std::fmt;

pub mod in_process {
    use super::fmt;

    pub fn prepared(id: impl fmt::Display, version: impl fmt::Display) -> String {
        format!("[stub] Module {id} v{version} prepared")
    }

    pub fn started(module: impl fmt::Display) -> String {
        format!("[stub] Module {module} started in-process")
    }

    pub fn stopped(module: impl fmt::Display) -> String {
        format!("[stub] Module {module} stopped")
    }

    pub fn restart_requested(module: impl fmt::Display) -> String {
        format!("[stub] Module {module} restart requested")
    }

    pub fn restarted(module: impl fmt::Display) -> String {
        format!("[stub] Module {module} restarted")
    }
}

pub mod process {
    use super::fmt;

    pub const STATE_DIR_PREPARE_FAILED: &str = "failed to prepare module runtime state directory";
    pub const NO_STATE_FOUND: &str = "no runtime state file found, starting fresh";
    pub fn state_loaded(count: usize) -> String {
        format!("loaded {} module states from disk", count)
    }
    pub const REATTACH_FAILED: &str = "failed to reattach running module";
    pub const PROCESS_MISSING: &str = "module was running but process no longer exists";
    pub fn state_parse_failed(err: impl fmt::Display) -> String {
        format!("failed to parse runtime state file: {err}")
    }
    pub fn state_read_failed(err: impl fmt::Display) -> String {
        format!("failed to read runtime state file: {err}")
    }
    pub const REATTACH_VERSION_MISMATCH: &str = "reattach version mismatch";
    pub const REATTACH_MISSING: &str = "installed module missing during runtime reattach";
    pub const REATTACH_RUNNING: &str = "reattached running module process";
    pub fn state_serialize_failed(err: impl fmt::Display) -> String {
        format!("failed to serialize state: {err}")
    }
    pub fn state_write_failed(err: impl fmt::Display) -> String {
        format!("failed to write state file: {err}")
    }
    pub fn state_persist_failed(err: impl fmt::Display) -> String {
        format!("failed to persist runtime state: {err}")
    }
    pub const MODULE_CONFIG_PORT: &str = "read port from module config";
    pub const MODULE_CONFIG_PARSE_FAILED: &str = "failed to parse module config.toml";
    pub const MODULE_CONFIG_MISSING: &str = "no config.toml found in module";
    pub fn exec_foreign_target(target: impl fmt::Display, current: impl fmt::Display) -> String {
        format!(
            "Modul-Binary ist für {} gebaut und nicht mit {} kompatibel",
            target, current
        )
    }
    pub fn exec_not_found(module_dir: impl fmt::Display, module_id: impl fmt::Display) -> String {
        format!(
            "kein ausführbares Modul in {} gefunden. Gesucht in: {}, bin/{}, {}.exe, bin/{}.exe",
            module_dir, module_id, module_id, module_id, module_id
        )
    }
    pub fn sigterm_failed(err: impl fmt::Display) -> String {
        format!("SIGTERM failed: {}, trying SIGKILL", err)
    }
    pub const PROCESS_STARTED: &str = "module process started";
    pub const MODULE_STOPPED: &str = "module stopped";
    pub fn log_dir_create_failed(err: impl fmt::Display) -> String {
        format!("failed to create log directory: {err}")
    }
    pub fn log_file_open_failed(err: impl fmt::Display) -> String {
        format!("failed to open log file: {err}")
    }
    pub fn log_file_read_failed(err: impl fmt::Display) -> String {
        format!("failed to read log file: {err}")
    }
    pub fn spawn_failed(err: impl fmt::Display) -> String {
        format!("failed to spawn process: {err}")
    }
    pub fn kill_failed(err: impl fmt::Display) -> String {
        format!("failed to kill process: {err}")
    }
}
