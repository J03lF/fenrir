use std::fmt;

pub const HISTORY_DIR_CREATE_FAILED: &str = "telemetry history directory could not be created";
pub const HISTORY_WRITE_FAILED: &str = "telemetry history could not be written";
pub const HISTORY_SERIALIZE_FAILED: &str = "telemetry history could not be serialized";
pub const HISTORY_PARSE_FAILED: &str = "telemetry history could not be parsed";
pub const HISTORY_READ_FAILED: &str = "telemetry history could not be read";
pub const ALREADY_INITIALIZED: &str = "telemetry already initialized";
pub const UPDATED: &str = "telemetry configuration updated";
pub const NOT_INITIALIZED: &str = "telemetry has not been initialized yet";
pub const READINESS_LOCK_POISONED: &str = "readiness probes lock poisoned";
pub const METRICS_DISABLED_CONFIG: &str =
    "process metrics disabled (telemetry.metrics.enabled=false)";
pub const METRICS_UNSUPPORTED_OS: &str = "process metrics not supported on this operating system";
pub const SYSTEM_METRICS_DISABLED: &str =
    "process metrics disabled (telemetry.system.enabled=false)";
pub const TELEMETRY_STATE_MISSING: &str =
    "telemetry state not initialized – process metrics will not be collected";
pub const PROCESS_METRICS_INIT_FAILED: &str = "failed to initialize process metrics";
pub const PROCESS_METRICS_SAMPLE_INITIAL_FAILED: &str = "initial process metrics sample failed";
pub const PROCESS_METRICS_SAMPLE_FAILED: &str = "process metrics sample failed";
pub const READINESS_PROBE_FAILED: &str = "readiness probe failed";
pub const PROCESS_MEMORY_SAMPLE_FAILED: &str = "process memory sample failed";
pub const PROCESS_CPU_SAMPLE_FAILED: &str = "process cpu sample failed";
pub const PROCESS_IO_SAMPLE_FAILED: &str = "process io sample failed";
pub const PROC_STAT_READ_FAILED: &str = "could not read /proc/self/stat";
pub const PROC_STAT_FORMAT_UNEXPECTED: &str = "/proc/self/stat format unexpected";
pub const PROC_STAT_FIELDS_MISSING: &str = "/proc/self/stat returned too few fields";
pub const PROC_STAT_UTIME_PARSE_FAILED: &str = "failed to parse utime";
pub const PROC_STAT_STIME_PARSE_FAILED: &str = "failed to parse stime";
pub const GETRUSAGE_FAILED: &str = "getrusage failed";
pub const CPU_METRICS_UNSUPPORTED: &str = "process cpu metrics not supported";
pub const PROC_STATM_READ_FAILED: &str = "could not read /proc/self/statm";
pub const STATM_TOTAL_MISSING: &str = "statm missing total size";
pub const STATM_TOTAL_PARSE_FAILED: &str = "failed to parse statm total";
pub const STATM_RESIDENT_MISSING: &str = "statm missing resident size";
pub const STATM_RESIDENT_PARSE_FAILED: &str = "failed to parse statm resident";
pub const MEMORY_METRICS_UNSUPPORTED: &str = "process memory metrics not supported";
pub const PROC_IO_OPEN_FAILED: &str = "could not open /proc/self/io";
pub const PROC_IO_READ_FAILED: &str = "failed to read /proc/self/io";
pub const READ_BYTES_PARSE_FAILED: &str = "failed to parse read_bytes";
pub const WRITE_BYTES_PARSE_FAILED: &str = "failed to parse write_bytes";
pub const CLK_TCK_INVALID: &str = "_SC_CLK_TCK returned an invalid value";
pub const PAGE_SIZE_INVALID: &str = "_SC_PAGESIZE returned an invalid value";

pub fn task_info_failed(error: impl fmt::Display) -> String {
    format!("task_info failed: {error}")
}
