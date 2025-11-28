use std::fmt;

pub mod command {
    pub const NAME: &str = "log";
    pub const DESCRIPTION: &str = "Öffnet einen Log-Stream in einem neuen Terminal";
    pub const USAGE: &str = "log [app|db|all|archive <ziel>|level <stufe>]";
    pub const DETAILS: &[&str] = &[
        "log              – streamt die aktuelle Applikationslogdatei",
        "log db           – streamt die DB-Logdatei",
        "log all          – öffnet App- und DB-Logs parallel",
        "log archive <ziel> – zeigt die letzte Archivdatei (ziel: app|db)",
        "log level <stufe> – setzt das Runtime-Loglevel (z. B. trace|debug|info|warn|error)",
    ];

    pub const SUB_APP_DESCRIPTION: &str = "App-Log streamen";
    pub const SUB_DB_DESCRIPTION: &str = "DB-Log streamen";
    pub const SUB_ALL_DESCRIPTION: &str = "App- und DB-Log öffnen";
    pub const SUB_LEVEL_DESCRIPTION: &str = "Loglevel zur Laufzeit aktualisieren";
    pub const SUB_ARCHIVE_DESCRIPTION: &str = "Neueste Archivdatei anzeigen";
}

pub mod handler {
    use super::*;

    pub const DEFAULT_LABEL_APP: &str = "App";
    pub const DEFAULT_LABEL_DB: &str = "DB";
    pub const ARCHIVE_LABEL_APP: &str = "App-Archiv";
    pub const ARCHIVE_LABEL_DB: &str = "DB-Archiv";
    pub const MISSING_LEVEL_USAGE: &str =
        "fehlender Wert. Nutzung: log level <trace|debug|info|warn|error>";
    pub const NO_RELOAD_HANDLE: &str =
        "Kein Logging-Reload-Handle vorhanden. SIGHUP oder CLI-Reload wird nicht unterstützt.";

    pub fn level_updated(level: &str) -> String {
        format!("Loglevel aktualisiert auf '{level}'.")
    }

    pub fn level_reload_failed(err: impl fmt::Display) -> String {
        format!("Konnte Loglevel nicht setzen: {err}")
    }

    pub fn unknown_archive_target(target: &str) -> String {
        format!("unbekanntes Archiv-Ziel: {target} (erlaubt: app|db)")
    }

    pub fn unknown_action(action: &str) -> String {
        format!("unbekanntes Ziel: {action}. Nutze 'log [app|db|all]', 'log archive [app|db]' oder 'log level <stufe>'.")
    }
}

pub mod launch {
    use super::*;

    pub fn log_file_line(label: &str, path: &std::path::Path) -> String {
        format!("[{label}] Logdatei: {}", path.display())
    }

    pub fn terminal_opened(label: &str) -> String {
        format!("[{label}] Terminal wurde geöffnet.")
    }

    pub fn terminal_failed(label: &str, path: &std::path::Path, err: impl fmt::Display) -> String {
        format!(
            "[{label}] Konnte kein Terminal starten ({err}). Führe manuell aus: tail -n 200 -f \"{}\"",
            path.display()
        )
    }

    pub fn no_log_available(label: &str) -> String {
        format!("[{label}] Keine Logdatei verfügbar.")
    }

    pub const UNSUPPORTED_OS: &str =
        "keine unterstützte Terminal-Integration für dieses Betriebssystem";
    pub const NO_TERMINAL_LAUNCHER: &str =
        "kein unterstütztes Terminalprogramm gefunden (z. B. gnome-terminal, konsole)";
    pub const WINDOWS_TAB_TITLE: &str = "Fenrir Logs";
}
