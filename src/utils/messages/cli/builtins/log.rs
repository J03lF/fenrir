use std::fmt;

pub mod command {
    pub const NAME: &str = "log";
    pub const DESCRIPTION: &str = "Opens a log stream in a new terminal";
    pub const USAGE: &str = "log [app|db|all|archive <target>|level <level>]";
    pub const DETAILS: &[&str] = &[
        "log              – streams the current application log file",
        "log db           – streams the database log file",
        "log all          – opens application and database logs side by side",
        "log archive <target> – shows the most recent archive file (target: app|db)",
        "log level <level> – sets the runtime log level (e.g. trace|debug|info|warn|error)",
    ];

    pub const SUB_APP_DESCRIPTION: &str = "Stream the application log";
    pub const SUB_DB_DESCRIPTION: &str = "Stream the database log";
    pub const SUB_ALL_DESCRIPTION: &str = "Open application and database logs";
    pub const SUB_LEVEL_DESCRIPTION: &str = "Update the log level at runtime";
    pub const SUB_ARCHIVE_DESCRIPTION: &str = "Show the latest archive file";
}

pub mod handler {
    use super::*;

    pub const DEFAULT_LABEL_APP: &str = "App";
    pub const DEFAULT_LABEL_DB: &str = "DB";
    pub const ARCHIVE_LABEL_APP: &str = "App archive";
    pub const ARCHIVE_LABEL_DB: &str = "DB archive";
    pub const MISSING_LEVEL_USAGE: &str =
        "missing value. Usage: log level <trace|debug|info|warn|error>";
    pub const NO_RELOAD_HANDLE: &str =
        "No logging reload handle is available. SIGHUP or CLI reload is not supported.";

    pub fn level_updated(level: &str) -> String {
        format!("Log level updated to '{level}'.")
    }

    pub fn level_reload_failed(err: impl fmt::Display) -> String {
        format!("Failed to set log level: {err}")
    }

    pub fn unknown_archive_target(target: &str) -> String {
        format!("unknown archive target: {target} (allowed: app|db)")
    }

    pub fn unknown_action(action: &str) -> String {
        format!("unknown target: {action}. Use 'log [app|db|all]', 'log archive [app|db]' or 'log level <level>'.")
    }
}

pub mod launch {
    use super::*;

    pub fn log_file_line(label: &str, path: &std::path::Path) -> String {
        format!("[{label}] Log file: {}", path.display())
    }

    pub fn terminal_opened(label: &str) -> String {
        format!("[{label}] Terminal opened.")
    }

    pub fn terminal_failed(label: &str, path: &std::path::Path, err: impl fmt::Display) -> String {
        format!(
            "[{label}] Failed to launch a terminal ({err}). Run manually: tail -n 200 -f \"{}\"",
            path.display()
        )
    }

    pub fn no_log_available(label: &str) -> String {
        format!("[{label}] No log file available.")
    }

    pub const UNSUPPORTED_OS: &str = "no supported terminal integration for this operating system";
    pub const NO_TERMINAL_LAUNCHER: &str =
        "no supported terminal program found (e.g. gnome-terminal, konsole)";
    pub const WINDOWS_TAB_TITLE: &str = "Fenrir Logs";
}
