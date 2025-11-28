use std::fmt;

pub mod archive {
    use super::fmt;

    pub fn create_dir_failed(path: impl fmt::Display) -> String {
        format!("konnte Log-Archiv nicht anlegen: {}", path)
    }

    pub fn rotate_move_failed(from: impl fmt::Display, to: impl fmt::Display) -> String {
        format!(
            "konnte Logdatei nicht in Archiv verschieben: {} -> {}",
            from, to
        )
    }
}

pub mod db {
    use super::fmt;

    pub fn init_failed(path: impl fmt::Display) -> String {
        format!("konnte DB-Log nicht initialisieren: {}", path)
    }
}

pub mod init {
    pub const FILE_LOGGING_INITIALIZED: &str = "dateilogging initialisiert";
    pub const STDOUT_LOGGING_INITIALIZED: &str = "logging auf stdout initialisiert";
    pub const DB_LOG_PREPARED: &str = "db-logdatei vorbereitet";

    use super::fmt;

    pub fn tracing_subscriber_init_failed(err: impl fmt::Display) -> String {
        format!("konnte Tracing-Subscriber nicht initialisieren: {err}")
    }
}

pub mod handle {
    use super::fmt;

    pub fn invalid_level(level: impl fmt::Display, err: impl fmt::Display) -> String {
        format!("ungültiger tracing level '{}': {err}", level)
    }

    pub fn reload_failed(err: impl fmt::Display) -> String {
        format!("konnte logging filter nicht aktualisieren: {err}")
    }
}

pub mod writers {
    use super::fmt;

    pub const INVALID_LOG_PATH: &str = "ungültiger Log-Pfad";

    pub fn create_dir_failed(path: impl fmt::Display) -> String {
        format!("konnte Log-Verzeichnis nicht anlegen: {}", path)
    }
}
