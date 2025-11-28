pub mod module_guard {
    pub const AUTO_MANAGED_SUMMARY: &str = "Module werden automatisch verwaltet – nutze 'sync module' oder 'install distribution'; Module starten/stoppen selbst.";

    pub fn action_disabled_line(action: &str) -> String {
        format!(
            "'{} module' ist deaktiviert (Module verwalten sich selbst).",
            action
        )
    }
}

pub mod list_command {
    pub const DESCRIPTION: &str = "Listet Ressourcen (Services, Jobs, Module)";
    pub const SYNOPSIS: &str = "list <services|jobs|modules>";
    pub const DETAILS: &[&str] = &[
        "list services – zeigt registrierte Services",
        "list jobs – listet Scheduler-Jobs",
        "list modules – zeigt installierte Module (mit Runtime)",
    ];
}

pub mod actions {
    use super::metadata::LIST_USAGE_HINT;

    pub fn unknown_list_resource(value: &str) -> String {
        format!("unbekannte Ressource: {value}")
    }

    pub fn available_resources_hint() -> String {
        format!("verfügbar: {LIST_USAGE_HINT}")
    }

    pub fn list_argument_hint(resource: &str) -> String {
        format!("Hinweis: 'list {resource}' erwartet keine weiteren Argumente.")
    }

    pub fn unknown_action_resource(value: &str) -> String {
        format!("unbekannte Ressource: {value}")
    }

    pub const ACTION_RESOURCE_HINT: &str = "gültig: service | module";
}

pub mod metadata {
    use super::module_guard::AUTO_MANAGED_SUMMARY;

    pub const LIST_USAGE_HINT: &str = "list services|jobs|modules";

    pub const START_DESCRIPTION: &str = "Startet Services oder Module";
    pub const START_SYNOPSIS: &str = "start <service|module> <ziel>";
    pub const START_USAGE: &str = "Nutzung: start service <id|--all>  (Module starten automatisch)";
    pub const START_DETAILS: &[&str] = &[
        "start service <id|--all> – startet einen steuerbaren Service",
        AUTO_MANAGED_SUMMARY,
    ];

    pub const STOP_DESCRIPTION: &str = "Stoppt Services oder Module kontrolliert";
    pub const STOP_SYNOPSIS: &str = "stop <service|module> <ziel> [--force]";
    pub const STOP_USAGE: &str =
        "Nutzung: stop service <id|--all> [--force]  (Module stoppen automatisch)";
    pub const STOP_DETAILS: &[&str] = &[
        "stop service <id|--all> [--force] – stoppt einen Service",
        AUTO_MANAGED_SUMMARY,
    ];

    pub const RESTART_DESCRIPTION: &str = "Startet Services oder Module neu";
    pub const RESTART_SYNOPSIS: &str = "restart <service|module> <ziel> [--force]";
    pub const RESTART_USAGE: &str = "Nutzung: restart service <id|--all> [--force]  (Module werden ohne manuelle Steuerung verwaltet)";
    pub const RESTART_DETAILS: &[&str] = &[
        "restart service <id|--all> [--force] – Neustart von Services",
        AUTO_MANAGED_SUMMARY,
    ];
}

pub mod control {
    pub fn missing_service_id(usage: &str) -> String {
        format!("fehlende Service-ID. {usage}")
    }

    pub fn started(id: &str) -> String {
        format!("Service {id} gestartet.")
    }

    pub fn already_running(id: &str) -> String {
        format!("Service {id} läuft bereits.")
    }

    pub fn stopped(id: &str) -> String {
        format!("Service {id} gestoppt.")
    }

    pub fn already_stopped(id: &str) -> String {
        format!("Service {id} war bereits gestoppt.")
    }

    pub fn restarted(id: &str) -> String {
        format!("Service {id} neu gestartet.")
    }

    pub fn unexpected_outcome(id: &str, outcome: &str) -> String {
        format!("Service {id}: unerwartetes Ergebnis {outcome}")
    }

    pub fn unknown_service(id: &str) -> String {
        format!("Unbekannter Service: {id}")
    }

    pub fn not_controllable(id: &str) -> String {
        format!("Service {id} unterstützt keine Steuerung über diese CLI.")
    }

    pub fn force_required(id: &str) -> String {
        format!("Service {id} ist als kritisch markiert. --force erforderlich.")
    }

    pub fn core_locked(id: &str) -> String {
        format!(
            "Service {id} gehört zur core-Plattform und kann nicht gestoppt oder neu gestartet werden."
        )
    }

    pub fn operation_failed(err: &str) -> String {
        format!("Operation fehlgeschlagen: {err}")
    }

    pub const NO_CONTROLLABLE_SERVICES: &str = "Keine steuerbaren Services gefunden.";

    pub fn bulk_header(action: &str) -> String {
        format!("Ergebnisse für {action} service --all:")
    }

    pub fn bulk_success_line(id: &str, outcome: &str) -> String {
        format!("  - {id}: {outcome}")
    }

    pub fn bulk_failure_line(id: &str, err: &str) -> String {
        format!("  - {id}: Fehler ({err})")
    }
}

pub mod list {
    pub const NO_SERVICES: &str = "Keine Services registriert.";
    pub const NO_JOBS: &str = "Keine Scheduler-Jobs registriert.";

    pub const SERVICE_HEADERS: &[&str] = &[
        "ID",
        "Name",
        "Typ",
        "Tags",
        "Status",
        "Seit",
        "Beschreibung",
        "Hinweis",
    ];

    pub const JOB_HEADERS: &[&str] = &["ID", "Intervall", "Beschreibung", "Status"];

    pub const EMPTY_VALUE: &str = "-";
    pub const STATUS_ACTIVE: &str = "active";
    pub const STATUS_INACTIVE: &str = "inactive";
}
