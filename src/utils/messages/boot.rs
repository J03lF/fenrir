use std::fmt;

pub mod bootstrap {
    pub const INVALID_SPEC: &str = "invalid bootstrap module spec";
    pub const MODULE_READY: &str = "bootstrap module ready";
    pub const INSTALL_FAILED: &str = "bootstrap module install failed";
    pub const EMPTY_SPEC: &str = "empty module spec";
    pub const EMPTY_VERSION_SEGMENT: &str = "empty version segment";
}

pub mod errors {
    pub const MISSING_SECRET: &str = "missing required secret";
    pub const CONFIG_INVALID: &str = "configuration invalid";
    pub const CONFIG_FILE_NOT_FOUND: &str = "configuration file not found";
    pub const CONFIG_PROFILE_INVALID: &str = "configuration profile invalid";
    pub const CONFIG_LOAD_FAILED: &str = "failed to load configuration";
    pub const LOGGING_INIT_FAILED: &str = "failed to initialize logging";
    pub const TELEMETRY_INIT_FAILED: &str = "failed to initialize telemetry";
    pub const DB_RUNTIME_START_FAILED: &str = "failed to start embedded database runtime";
    pub const DB_ADAPTERS_FAILED: &str = "failed to build database adapters";
    pub const DEFAULT_DB_ENGINE_INVALID: &str = "invalid default database engine";
    pub const TELEMETRY_PROBE_FAILED: &str = "failed to register telemetry readiness probe";
    pub const DB_SHELL_INIT_FAILED: &str = "failed to initialize database shell service";
    pub const DB_MIGRATIONS_FAILED: &str = "failed to apply database migrations";
    pub const AUDIT_DIR_PREP_FAILED: &str = "failed to prepare audit storage directory";
    pub const AUDIT_PERSISTENCE_INIT_FAILED: &str = "failed to initialize audit persistence";
    pub const MODULE_REGISTRY_INIT_FAILED: &str = "failed to initialize module registry";
    pub const MODULE_STORAGE_INIT_FAILED: &str = "failed to initialize module storage";
    pub const MODULE_VERIFIER_INIT_FAILED: &str = "failed to initialize module verifier";
    pub const MODULE_SERVICE_ATTACH_FAILED: &str = "failed to attach module service";
    pub const TOKEN_EXCHANGE_ATTACH_FAILED: &str = "failed to attach token exchange service";
    pub const IDENTITY_PROVIDER_INIT_FAILED: &str = "failed to initialize identity provider";
    pub const IDENTITY_SERVICE_ATTACH_FAILED: &str = "failed to attach identity service";
    pub const SECURITY_MANAGER_INIT_FAILED: &str = "failed to initialize security manager";
    pub const SECURITY_MANAGER_ATTACH_FAILED: &str = "failed to attach security manager";
    pub const SESSION_SERVICE_ATTACH_FAILED: &str = "failed to attach session service";
    pub const SCHEDULER_JOBS_INSTALL_FAILED: &str = "failed to install scheduler jobs";
    pub const HTTP_SERVER_INIT_FAILED: &str = "failed to initialize http server";
    pub const MODULE_SERVICE_SCOPE_INVALID: &str =
        "invalid default service scopes configured for modules";
    pub const DB_CONNECTOR_INIT_FAILED: &str = "failed to initialize db connector";
}

pub mod helpers {
    pub const RUNTIME_DIR_CANDIDATE_FAILED: &str = "failed to create runtime directory candidate";
}

pub mod logs {
    pub const CORE_SERVICES_REGISTERED: &str = "core services registered in registry";
    pub const SCHEDULER_STARTED: &str = "scheduler service started";
    pub const AUDIT_PERSISTENCE_CONFIGURED: &str = "audit log persistence configured";
    pub const BOOT_COMPLETE: &str = "boot complete";
}

pub mod runtime {
    pub const STATE_RESTORE_FAILED: &str = "failed to restore module runtime state";
    pub const HANDLE_MISSING: &str = "tokio runtime not available, skipping module state restore";
}

pub mod services {
    pub mod names {
        pub const DB_SHELL: &str = "DB Shell Service";
        pub const SSH: &str = "SSH Transport";
        pub const CLI: &str = "Local CLI";
        pub const MODULE_RUNTIME: &str = "Module Runtime";
    pub const DB_RUNTIME: &str = "Database Runtime";
        pub const SCHEDULER: &str = "Background Scheduler";
        pub const HTTP: &str = "HTTP Transport";
        pub const IDENTITY: &str = "Identity Broker";
        pub const DB_CONNECTOR: &str = "DB Connector";
        pub const TOKEN_EXCHANGE: &str = "Token Exchange";
        pub const MODULE_LIFECYCLE: &str = "Module Lifecycle";
        pub const JOBS_CONTROL: &str = "Jobs Control";
    }

    pub mod descriptions {
        pub const DB_SHELL: &str = "Interactive database subshell for admin commands";
        pub const SSH: &str = "Secure shell access and interactive sessions";
        pub const CLI: &str = "Interactive CLI shell (local)";
        pub const MODULE_RUNTIME: &str = "Manages installed CLI modules";
        pub const SCHEDULER: &str = "Manages periodic jobs and tasks";
        pub const HTTP: &str = "REST API, health, and telemetry";
        pub const IDENTITY: &str = "Issues and validates control-plane tokens";
        pub const DB_CONNECTOR: &str = "Proxies module database requests over IPC";
        pub const TOKEN_EXCHANGE: &str = "Issues delegated tokens for managed modules";
        pub const MODULE_LIFECYCLE: &str = "Operator-facing module lifecycle orchestrator";
        pub const JOBS_CONTROL: &str = "Front-door for scheduler actions and job controls";
    pub const DB_RUNTIME: &str = "Embedded database runtime supervisor";
    }

    pub mod notes {
        pub const READY: &str = "ready";
        pub const INITIALIZATION: &str = "initializing";
        pub const WAITING_FOR_INVOKE: &str = "waiting for invoke";
        pub const NO_MODULES: &str = "no modules installed";
        pub const HTTP_WAITING: &str = "waiting to start";
        pub const HTTP_DISABLED: &str = "disabled (enable_http=false)";
        pub const MODULE_READY: &str = "ready for modules";
        pub const DB_SHELL_ENABLED: &str = "db-shell enabled";
        pub const DB_SHELL_DISABLED: &str = "db-shell disabled";
        pub const CLI_READY_FOR_SESSIONS: &str = "ready for new sessions";
        pub const CLI_DISABLED: &str = "cli disabled";
        pub const TOKEN_EXCHANGE_READY: &str = "token exchange ready";
        pub const MODULE_LIFECYCLE_READY: &str = "module lifecycle orchestrator active";
        pub const JOBS_CONTROL_READY: &str = "jobs control surface active";
    pub const DB_RUNTIME_READY: &str = "embedded db runtime active";
    }
}

pub mod transports {
    use super::*;

    pub const MODULE_SERVICE_NOT_ATTACHED: &str =
        "module service not attached; skipping bootstrap modules";
    pub const SSH_TASK_SPAWNED: &str = "ssh server task spawned";
    pub const SSH_SERVER_EXITED_WITH_ERROR: &str = "ssh server exited with error";
    pub const HTTP_SERVER_START_FAILED: &str = "http server start failed";
    pub const TRANSPORT_INITIALISATION_TRIGGERED: &str = "transport initialisation triggered";
    pub const SSH_STARTING_NOTE: &str = "starting listener";
    pub const HTTP_STARTING_NOTE: &str = "initializing";

    pub fn ssh_failure_note(err: impl fmt::Display) -> String {
        format!("error: {err}")
    }

    pub fn http_start_failure_note(err: impl fmt::Display) -> String {
        format!("startup error: {err}")
    }
}

pub mod config_reload {
    pub const SIGNAL_HANDLER_INIT_FAILED: &str = "failed to initialize SIGHUP signal handler";
    pub const LOG_LEVEL_UPDATE_FAILED: &str = "failed to update logging level";
    pub const TLS_RELOAD_FAILED: &str = "TLS reload failed";
    pub const CONFIG_RELOADED: &str = "configuration (logging/telemetry/TLS) reloaded";
    pub const CONFIG_RELOAD_FAILED: &str = "configuration reload failed";
    pub const HOT_RELOAD_UNSUPPORTED: &str =
        "config hot reload is not supported on this platform; skipping SIGHUP replay";
}
