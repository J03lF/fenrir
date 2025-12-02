use std::fmt;

pub mod archive {
    use super::fmt;

    pub fn create_dir_failed(path: impl fmt::Display) -> String {
        format!("failed to prepare log archive: {}", path)
    }

    pub fn rotate_move_failed(from: impl fmt::Display, to: impl fmt::Display) -> String {
        format!("failed to move log file into archive: {} -> {}", from, to)
    }
}

pub mod db {
    use super::fmt;

    pub fn init_failed(path: impl fmt::Display) -> String {
        format!("failed to initialize db log: {}", path)
    }
}

pub mod init {
    pub const FILE_LOGGING_INITIALIZED: &str = "file logging initialized";
    pub const STDOUT_LOGGING_INITIALIZED: &str = "stdout logging initialized";
    pub const DB_LOG_PREPARED: &str = "db log file prepared";

    use super::fmt;

    pub fn tracing_subscriber_init_failed(err: impl fmt::Display) -> String {
        format!("failed to initialize tracing subscriber: {err}")
    }
}

pub mod handle {
    use super::fmt;

    pub fn invalid_level(level: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("invalid tracing level '{}': {err}", level)
    }

    pub fn reload_failed(err: impl fmt::Display) -> String {
        format!("failed to update logging filter: {err}")
    }
}

pub mod writers {
    use super::fmt;

    pub const INVALID_LOG_PATH: &str = "invalid log path";

    pub fn create_dir_failed(path: impl fmt::Display) -> String {
        format!("failed to create log directory: {}", path)
    }
}
