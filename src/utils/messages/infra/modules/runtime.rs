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
    pub fn runtime_manifest_read_failed(err: impl fmt::Display) -> String {
        format!("failed to read runtime manifest: {err}")
    }
    pub fn runtime_manifest_parse_failed(err: impl fmt::Display) -> String {
        format!("failed to parse runtime manifest: {err}")
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
            "module binary was built for {} and is not compatible with {}",
            target, current
        )
    }
    pub fn exec_not_found(module_dir: impl fmt::Display, module_id: impl fmt::Display) -> String {
        format!(
            "no executable module found in {}. Searched: {}, bin/{}, {}.exe, bin/{}.exe",
            module_dir, module_id, module_id, module_id, module_id
        )
    }
    pub fn sigterm_failed(err: impl fmt::Display) -> String {
        format!("SIGTERM failed: {}, trying SIGKILL", err)
    }
    pub const SIGTERM_TIMEOUT: &str = "module process still running after SIGTERM; sending SIGKILL";
    pub const PROCESS_EXIT_TIMEOUT: &str = "module process did not exit after forced termination";
    pub const PROCESS_STARTED: &str = "module process started";
    pub const MODULE_STOPPED: &str = "module stopped";
    pub const STATIC_SERVER_STARTED: &str = "module static server started";
    pub const STATIC_SERVER_STOPPED: &str = "module static server stopped";
    pub fn log_dir_create_failed(err: impl fmt::Display) -> String {
        format!("failed to create log directory: {err}")
    }
    pub fn log_file_reset_failed(err: impl fmt::Display) -> String {
        format!("failed to reset log file: {err}")
    }
    pub fn log_file_open_failed(err: impl fmt::Display) -> String {
        format!("failed to open log file: {err}")
    }
    pub fn log_file_read_failed(err: impl fmt::Display) -> String {
        format!("failed to read log file: {err}")
    }
    pub fn log_file_write_failed(err: impl fmt::Display) -> String {
        format!("failed to write module log line: {err}")
    }
    pub fn static_entrypoint_missing(path: impl fmt::Display) -> String {
        format!("declared static entrypoint {path} does not exist")
    }
    pub fn static_asset_roots_missing() -> String {
        "runtime.static_site.asset_roots must declare at least one directory".to_string()
    }
    pub fn static_assets_not_found(root: impl fmt::Display) -> String {
        format!("no static assets with index.html were found under {}", root)
    }
    pub fn static_services_manifest_read_failed(err: impl fmt::Display) -> String {
        format!("failed to read static services manifest: {err}")
    }
    pub fn static_services_manifest_invalid(err: impl fmt::Display) -> String {
        format!("static services manifest is invalid JSON: {err}")
    }
    pub fn spawn_failed(err: impl fmt::Display) -> String {
        format!("failed to spawn process: {err}")
    }
    pub fn kill_failed(err: impl fmt::Display) -> String {
        format!("failed to kill process: {err}")
    }
    pub const BOOTSTRAP_BINARY_MISSING: &str =
        "fenrir-module-kit bootstrap binary is missing next to the fenrir executable";
    pub fn bootstrap_exe_resolve_failed(err: impl fmt::Display) -> String {
        format!("failed to resolve bootstrap executable: {err}")
    }
    pub fn bootstrap_spawn_failed(err: impl fmt::Display) -> String {
        format!("failed to run module bootstrapper: {err}")
    }
    pub fn bootstrap_failed(status: impl fmt::Display) -> String {
        format!("module bootstrapper exited with status {status}")
    }
}
