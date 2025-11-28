use std::fmt;

pub const HISTORY_DIR_CREATE_FAILED: &str =
    "telemetrie-history verzeichnis konnte nicht erstellt werden";
pub const HISTORY_WRITE_FAILED: &str = "telemetrie-history konnte nicht geschrieben werden";
pub const HISTORY_SERIALIZE_FAILED: &str = "telemetrie-history konnte nicht serialisiert werden";
pub const HISTORY_PARSE_FAILED: &str = "telemetrie-history konnte nicht geparst werden";
pub const HISTORY_READ_FAILED: &str = "telemetrie-history konnte nicht gelesen werden";
pub const ALREADY_INITIALIZED: &str = "telemetry bereits initialisiert";
pub const UPDATED: &str = "telemetry-konfiguration aktualisiert";
pub const NOT_INITIALIZED: &str = "telemetry wurde noch nicht initialisiert";
pub const READINESS_LOCK_POISONED: &str = "readiness probes lock poisoned";
pub const METRICS_DISABLED_CONFIG: &str =
    "prozessmetriken deaktiviert (telemetry.metrics.enabled=false)";
pub const METRICS_UNSUPPORTED_OS: &str =
    "prozessmetriken werden auf diesem betriebssystem nicht unterstützt";
pub const SYSTEM_METRICS_DISABLED: &str =
    "prozessmetriken deaktiviert (telemetry.system.enabled=false)";
pub const TELEMETRY_STATE_MISSING: &str =
    "telemetry state nicht initialisiert – prozessmetriken werden nicht erhoben";
pub const PROCESS_METRICS_INIT_FAILED: &str = "prozessmetriken konnten nicht initialisiert werden";
pub const PROCESS_METRICS_SAMPLE_INITIAL_FAILED: &str =
    "initiales prozessmetriken-sample fehlgeschlagen";
pub const PROCESS_METRICS_SAMPLE_FAILED: &str = "prozessmetriken sample fehlgeschlagen";
pub const READINESS_PROBE_FAILED: &str = "readiness probe failed";
pub const PROCESS_MEMORY_SAMPLE_FAILED: &str = "prozess speicher sample fehlgeschlagen";
pub const PROCESS_CPU_SAMPLE_FAILED: &str = "prozess cpu sample fehlgeschlagen";
pub const PROCESS_IO_SAMPLE_FAILED: &str = "prozess io sample fehlgeschlagen";
pub const PROC_STAT_READ_FAILED: &str = "/proc/self/stat konnte nicht gelesen werden";
pub const PROC_STAT_FORMAT_UNEXPECTED: &str = "/proc/self/stat format unerwartet";
pub const PROC_STAT_FIELDS_MISSING: &str = "/proc/self/stat liefert zu wenige felder";
pub const PROC_STAT_UTIME_PARSE_FAILED: &str = "utime konnte nicht geparst werden";
pub const PROC_STAT_STIME_PARSE_FAILED: &str = "stime konnte nicht geparst werden";
pub const GETRUSAGE_FAILED: &str = "getrusage fehlgeschlagen";
pub const CPU_METRICS_UNSUPPORTED: &str = "prozess cpu metriken nicht unterstützt";
pub const PROC_STATM_READ_FAILED: &str = "/proc/self/statm konnte nicht gelesen werden";
pub const STATM_TOTAL_MISSING: &str = "statm enthält keine größe";
pub const STATM_TOTAL_PARSE_FAILED: &str = "statm total konnte nicht geparst werden";
pub const STATM_RESIDENT_MISSING: &str = "statm enthält keine resident größe";
pub const STATM_RESIDENT_PARSE_FAILED: &str = "statm resident konnte nicht geparst werden";
pub const MEMORY_METRICS_UNSUPPORTED: &str = "prozess speichermetriken nicht unterstützt";
pub const PROC_IO_OPEN_FAILED: &str = "/proc/self/io konnte nicht geöffnet werden";
pub const PROC_IO_READ_FAILED: &str = "Fehler beim Lesen von /proc/self/io";
pub const READ_BYTES_PARSE_FAILED: &str = "read_bytes konnte nicht geparst werden";
pub const WRITE_BYTES_PARSE_FAILED: &str = "write_bytes konnte nicht geparst werden";
pub const CLK_TCK_INVALID: &str = "_SC_CLK_TCK liefert ungültigen wert";
pub const PAGE_SIZE_INVALID: &str = "_SC_PAGESIZE liefert ungültigen wert";

pub fn task_info_failed(error: impl fmt::Display) -> String {
    format!("task_info fehlgeschlagen: {error}")
}
