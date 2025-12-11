pub mod module_guard {
    pub const AUTO_MANAGED_SUMMARY: &str =
        "Modules share the same runtime driver – 'start|stop|restart module <name>' is a verb-first alias for 'modules <verb> <name>'.";
}

pub mod list_command {
    pub const DESCRIPTION: &str = "List resources (services, jobs, modules)";
    pub const SYNOPSIS: &str = "list <services|jobs|modules>";
    pub const DETAILS: &[&str] = &[
        "list services – show registered services",
        "list jobs – list scheduler jobs",
        "list modules – show installed modules (with runtime state)",
    ];
}

pub mod actions {
    use super::metadata::LIST_USAGE_HINT;

    pub fn unknown_list_resource(value: &str) -> String {
        format!("unknown resource: {value}")
    }

    pub fn available_resources_hint() -> String {
        format!("available: {LIST_USAGE_HINT}")
    }

    pub fn list_argument_hint(resource: &str) -> String {
        format!("Hint: 'list {resource}' does not take additional arguments.")
    }

    pub fn unknown_action_resource(value: &str) -> String {
        format!("unknown resource: {value}")
    }

    pub const ACTION_RESOURCE_HINT: &str = "valid: service | module | job (restart only)";

    pub fn missing_job_id() -> String {
        "missing job id. Usage: restart job <id>".to_string()
    }

    pub fn job_action_unsupported(action: &str) -> String {
        format!("'{action} job' is not supported.")
    }
}

pub mod metadata {
    use super::module_guard::AUTO_MANAGED_SUMMARY;

    pub const LIST_USAGE_HINT: &str = "list services|jobs|modules";

    pub const START_DESCRIPTION: &str = "Start services or modules";
    pub const START_SYNOPSIS: &str = "start <service|module> <target>";
    pub const START_USAGE: &str = "Usage: start service <id|--all>  | start module <name>";
    pub const START_DETAILS: &[&str] = &[
        "start service <id|--all> – start a controllable service",
        AUTO_MANAGED_SUMMARY,
    ];

    pub const STOP_DESCRIPTION: &str = "Stop services or modules";
    pub const STOP_SYNOPSIS: &str = "stop <service|module> <target> [--force]";
    pub const STOP_USAGE: &str = "Usage: stop service <id|--all> [--force]  | stop module <name>";
    pub const STOP_DETAILS: &[&str] = &[
        "stop service <id|--all> [--force] – stop a service",
        AUTO_MANAGED_SUMMARY,
    ];

    pub const RESTART_DESCRIPTION: &str = "Restart services or modules";
    pub const RESTART_SYNOPSIS: &str = "restart <service|module> <target> [--force]";
    pub const RESTART_USAGE: &str =
        "Usage: restart service <id|--all> [--force]  | restart module <name>";
    pub const RESTART_DETAILS: &[&str] = &[
        "restart service <id|--all> [--force] – restart services",
        AUTO_MANAGED_SUMMARY,
    ];

    pub const PAUSE_DESCRIPTION: &str = "Pause scheduler jobs";
    pub const PAUSE_SYNOPSIS: &str = "pause job <id>";
    pub const PAUSE_USAGE: &str = "Usage: pause job <job-id>";
    pub const PAUSE_DETAILS: &[&str] = &["pause job <id> – stops a scheduler job until resumed."];

    pub const RESUME_DESCRIPTION: &str = "Resume paused scheduler jobs";
    pub const RESUME_SYNOPSIS: &str = "resume job <id>";
    pub const RESUME_USAGE: &str = "Usage: resume job <job-id>";
    pub const RESUME_DETAILS: &[&str] =
        &["resume job <id> – re-enables a previously paused scheduler job."];
}

pub mod control {
    pub fn missing_service_id(usage: &str) -> String {
        format!("missing service id. {usage}")
    }

    pub fn started(id: &str) -> String {
        format!("Service {id} started.")
    }

    pub fn already_running(id: &str) -> String {
        format!("Service {id} is already running.")
    }

    pub fn stopped(id: &str) -> String {
        format!("Service {id} stopped.")
    }

    pub fn already_stopped(id: &str) -> String {
        format!("Service {id} was already stopped.")
    }

    pub fn restarted(id: &str) -> String {
        format!("Service {id} restarted.")
    }

    pub fn unexpected_outcome(id: &str, outcome: &str) -> String {
        format!("Service {id}: unexpected outcome {outcome}")
    }

    pub fn unknown_service(id: &str) -> String {
        format!("Unknown service: {id}")
    }

    pub fn not_controllable(id: &str) -> String {
        format!("Service {id} cannot be controlled via this CLI.")
    }

    pub fn force_required(id: &str) -> String {
        format!("Service {id} is marked as critical. --force required.")
    }

    pub fn core_locked(id: &str) -> String {
        format!("Service {id} is part of the core platform and cannot be stopped or restarted.")
    }

    pub fn operation_failed(err: &str) -> String {
        format!("Operation failed: {err}")
    }

    pub const NO_CONTROLLABLE_SERVICES: &str = "No controllable services found.";

    pub fn bulk_header(action: &str) -> String {
        format!("Results for {action} service --all:")
    }

    pub fn bulk_success_line(id: &str, outcome: &str) -> String {
        format!("  - {id}: {outcome}")
    }

    pub fn bulk_failure_line(id: &str, err: &str) -> String {
        format!("  - {id}: error ({err})")
    }
}

pub mod list {
    pub const NO_SERVICES: &str = "No services registered.";
    pub const NO_JOBS: &str = "No scheduler jobs registered.";

    pub const SERVICE_HEADERS: &[&str] = &[
        "ID",
        "Name",
        "Type",
        "Tags",
        "Status",
        "Since",
        "Description",
        "Note",
    ];

    pub const JOB_HEADERS: &[&str] = &["ID", "Interval", "Description", "Status"];

    pub const EMPTY_VALUE: &str = "-";
    pub const STATUS_ACTIVE: &str = "active";
    pub const STATUS_INACTIVE: &str = "inactive";
    pub const STATUS_PAUSED: &str = "paused";
    pub const HEALTH_HEALTHY: &str = "healthy";
    pub const HEALTH_DEGRADED: &str = "degraded";
    pub const HEALTH_STALE: &str = "stale";
    pub const HEALTH_UNKNOWN: &str = "unknown";
}
