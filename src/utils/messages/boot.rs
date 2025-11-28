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
    pub const DB_ADAPTERS_FAILED: &str = "failed to build database adapters";
    pub const DEFAULT_DB_ENGINE_INVALID: &str = "invalid default database engine";
    pub const TELEMETRY_PROBE_FAILED: &str = "failed to register telemetry readiness probe";
    pub const DB_SHELL_INIT_FAILED: &str = "failed to initialize database shell service";
    pub const AUDIT_DIR_PREP_FAILED: &str = "failed to prepare audit storage directory";
    pub const AUDIT_PERSISTENCE_INIT_FAILED: &str = "failed to initialize audit persistence";
    pub const MODULE_REGISTRY_INIT_FAILED: &str = "failed to initialize module registry";
    pub const MODULE_STORAGE_INIT_FAILED: &str = "failed to initialize module storage";
    pub const MODULE_VERIFIER_INIT_FAILED: &str = "failed to initialize module verifier";
    pub const MODULE_SERVICE_ATTACH_FAILED: &str = "failed to attach module service";
    pub const IDENTITY_PROVIDER_INIT_FAILED: &str = "failed to initialize identity provider";
    pub const IDENTITY_SERVICE_ATTACH_FAILED: &str = "failed to attach identity service";
    pub const SECURITY_MANAGER_INIT_FAILED: &str = "failed to initialize security manager";
    pub const SECURITY_MANAGER_ATTACH_FAILED: &str = "failed to attach security manager";
    pub const SESSION_SERVICE_ATTACH_FAILED: &str = "failed to attach session service";
    pub const SCHEDULER_JOBS_INSTALL_FAILED: &str = "failed to install scheduler jobs";
    pub const HTTP_SERVER_INIT_FAILED: &str = "failed to initialize http server";
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
        pub const CLI: &str = "Lokale CLI";
        pub const MODULE_RUNTIME: &str = "Module Runtime";
        pub const SCHEDULER: &str = "Background Scheduler";
        pub const HTTP: &str = "HTTP Transport";
        pub const IDENTITY: &str = "Identity Broker";
    }

    pub mod descriptions {
        pub const DB_SHELL: &str = "Interaktive Datenbank-Subshell für Admin-Kommandos";
        pub const SSH: &str = "Secure Shell Zugang und interaktive Sitzungen";
        pub const CLI: &str = "Interaktive CLI-Shell (lokal)";
        pub const MODULE_RUNTIME: &str = "Verwaltet installierte CLI-Module";
        pub const SCHEDULER: &str = "Verwaltet periodische Jobs und Tasks";
        pub const HTTP: &str = "REST-API, Health und Telemetrie";
        pub const IDENTITY: &str = "Ausstellung und Prüfung von Control-Plane-Token";
    }

    pub mod notes {
        pub const READY: &str = "bereit";
        pub const INITIALIZATION: &str = "Initialisierung";
        pub const WAITING_FOR_INVOKE: &str = "Wartend auf Aufruf";
        pub const NO_MODULES: &str = "Keine Module installiert";
        pub const HTTP_WAITING: &str = "wartet auf Start";
        pub const HTTP_DISABLED: &str = "deaktiviert (enable_http=false)";
        pub const MODULE_READY: &str = "Bereit für Module";
        pub const DB_SHELL_ENABLED: &str = "DB-Shell aktiviert";
        pub const DB_SHELL_DISABLED: &str = "DB-Shell deaktiviert";
        pub const CLI_READY_FOR_SESSIONS: &str = "Bereit für neue Sessions";
        pub const CLI_DISABLED: &str = "CLI deaktiviert";
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
    pub const SSH_STARTING_NOTE: &str = "Starte Listener";
    pub const HTTP_STARTING_NOTE: &str = "initialisiere";

    pub fn ssh_failure_note(err: impl fmt::Display) -> String {
        format!("Fehler: {err}")
    }

    pub fn http_start_failure_note(err: impl fmt::Display) -> String {
        format!("Start-Fehler: {err}")
    }
}

pub mod config_reload {
    pub const SIGNAL_HANDLER_INIT_FAILED: &str =
        "konnte SIGHUP-Signal-Handler nicht initialisieren";
    pub const LOG_LEVEL_UPDATE_FAILED: &str = "konnte Logging-Level nicht aktualisieren";
    pub const TLS_RELOAD_FAILED: &str = "TLS-Reload fehlgeschlagen";
    pub const CONFIG_RELOADED: &str = "Konfiguration (Logging/Telemetry/TLS) neu geladen";
    pub const CONFIG_RELOAD_FAILED: &str = "Konfigurations-Reload fehlgeschlagen";
    pub const HOT_RELOAD_UNSUPPORTED: &str =
        "Config-Hot-Reload wird auf dieser Plattform nicht unterstützt; SIGHUP-Replay übersprungen";
}
