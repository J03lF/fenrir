pub struct ProblemText {
    pub code: &'static str,
    pub message: String,
}

impl ProblemText {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub mod problems {
    use super::ProblemText;
    use std::fmt;

    pub fn module_service_unavailable() -> ProblemText {
        ProblemText::new(
            "module_service_unavailable",
            "Modul-Service nicht verfügbar",
        )
    }

    pub fn identity_service_unavailable() -> ProblemText {
        ProblemText::new(
            "identity_service_unavailable",
            "Identity-Service nicht verfügbar",
        )
    }

    pub fn invalid_role(value: &str) -> ProblemText {
        ProblemText::new("invalid_role", format!("Unbekannte Rolle '{value}'"))
    }

    pub fn role_insufficient_viewer() -> ProblemText {
        ProblemText::new("role_insufficient", "Mindestens Rolle viewer erforderlich")
    }

    pub fn role_insufficient_admin() -> ProblemText {
        ProblemText::new("role_insufficient", "Aktion erfordert Rolle admin")
    }

    pub fn role_insufficient(required: &str, actual: &str) -> ProblemText {
        ProblemText::new(
            "role_insufficient",
            format!("Aktion erfordert Rolle {required}, aktuelle Rolle {actual} reicht nicht aus"),
        )
    }

    pub fn audit_unavailable(err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "audit_unavailable",
            format!("Audit-Store nicht verfügbar: {err}"),
        )
    }

    pub fn invalid_level_empty() -> ProblemText {
        ProblemText::new("invalid_level", "Loglevel darf nicht leer sein")
    }

    pub fn logging_reload_unavailable() -> ProblemText {
        ProblemText::new(
            "logging_reload_unavailable",
            "Kein Logging-Reload-Handle registriert",
        )
    }

    pub fn logging_reload_failed(err: impl fmt::Display) -> ProblemText {
        ProblemText::new(
            "logging_reload_failed",
            format!("Loglevel konnte nicht gesetzt werden: {err}"),
        )
    }

    pub fn identity_list_failed() -> ProblemText {
        ProblemText::new(
            "identity_list_failed",
            "Identity-Benutzer konnten nicht geladen werden",
        )
    }

    pub fn identity_task_failed() -> ProblemText {
        ProblemText::new(
            "identity_task_failed",
            "Identity-Benutzer konnten nicht geladen werden",
        )
    }

    pub fn identity_issue_failed() -> ProblemText {
        ProblemText::new(
            "identity_issue_failed",
            "Token konnte nicht ausgestellt werden",
        )
    }

    pub fn identity_issue_task_failed() -> ProblemText {
        ProblemText::new(
            "identity_task_failed",
            "Token konnte nicht ausgestellt werden",
        )
    }

    pub fn service_unknown(id: &str) -> ProblemText {
        ProblemText::new(
            "unknown_service",
            format!("Service `{id}` ist nicht registriert."),
        )
    }

    pub fn service_not_controllable(id: &str) -> ProblemText {
        ProblemText::new(
            "not_controllable",
            format!("Service `{id}` lässt sich nicht über HTTP steuern."),
        )
    }

    pub fn service_force_required(id: &str) -> ProblemText {
        ProblemText::new(
            "force_required",
            format!(
                "Service `{id}` ist als kritisch markiert. Bitte Aktion mit force=true bestätigen."
            ),
        )
    }

    pub fn service_core_locked(id: &str) -> ProblemText {
        ProblemText::new(
            "core_locked",
            format!(
                "Service `{id}` gehört zur core-Plattform und kann nicht gestoppt oder neu gestartet werden."
            ),
        )
    }

    pub fn service_operation_failed(err: impl fmt::Display) -> ProblemText {
        ProblemText::new("operation_failed", format!("Aktion fehlgeschlagen: {err}"))
    }

    pub fn unauthorized() -> ProblemText {
        ProblemText::new("unauthorized", "Autorisierung erforderlich")
    }

    pub fn forbidden() -> ProblemText {
        ProblemText::new("forbidden", "Zugriff verweigert")
    }
}

pub mod tls {
    use std::fmt;

    pub const CERT_PATH_REQUIRED: &str =
        "server.http.tls.cert_path muss gesetzt sein, wenn TLS aktiviert ist";
    pub const KEY_PATH_REQUIRED: &str =
        "server.http.tls.key_path muss gesetzt sein, wenn TLS aktiviert ist";
    pub const RUNTIME_LOCK_POISONED: &str = "tls runtime lock poisoned";
    pub const WATCHER_LOCK_POISONED: &str = "tls watcher lock poisoned";
    pub const RELOAD_LOCK_POISONED: &str = "tls reload lock poisoned";
    pub const FILE_WATCHER_CREATE_FAILED: &str = "TLS-Datei-Watcher konnte nicht erstellt werden";
    pub const CERT_WATCH_FAILED: &str = "TLS-Zertifikat kann nicht beobachtet werden";
    pub const KEY_WATCH_FAILED: &str = "TLS-Schlüssel kann nicht beobachtet werden";
    pub const RELOAD_FAILED: &str = "TLS-Zertifikate konnten nicht neu geladen werden";
    pub const RELOAD_SUCCESS_FILESYSTEM: &str = "TLS-Zertifikate neu geladen (Filesystem-Event)";
    pub const WATCH_ERROR: &str = "Fehler beim Beobachten der TLS-Artefakte";
    pub const REQUEST_FAILED: &str = "TLS request handling failed";
    pub const INTERNAL_SERVER_ERROR_BODY: &str = "internal server error";
    pub const HOOKS_REGISTER_POISONED: &str =
        "TLS reload hooks poisoned; unable to register new hook";
    pub const HOOKS_NOTIFY_POISONED: &str = "TLS reload hooks poisoned; skipping notifications";
    pub const HOOK_PANICKED: &str = "TLS reload hook panicked";
    pub const CERT_PARSE_FAILED: &str = "Zertifikat konnte nicht geparst werden";
    pub const CERT_CHAIN_EMPTY: &str = "keine Zertifikatsketten gefunden";
    pub const PKCS8_PARSE_FAILED: &str = "Privater Schlüssel (PKCS8) konnte nicht geparst werden";
    pub const RSA_PARSE_FAILED: &str = "Privater Schlüssel (RSA) konnte nicht geparst werden";
    pub const PRIVATE_KEY_MISSING: &str = "keinen privaten Schlüssel gefunden";

    pub fn cert_not_found(path: impl fmt::Display) -> String {
        format!("TLS-Zertifikat '{path}' wurde nicht gefunden")
    }

    pub fn key_not_found(path: impl fmt::Display) -> String {
        format!("TLS-Schlüssel '{path}' wurde nicht gefunden")
    }

    pub fn cert_read_failed(path: impl fmt::Display) -> String {
        format!("konnte Zertifikat '{path}' nicht lesen")
    }

    pub fn key_read_failed(path: impl fmt::Display) -> String {
        format!("konnte Schlüssel '{path}' nicht lesen")
    }

    pub fn cipher_suite_unknown(name: &str) -> String {
        format!("unbekannte Cipher Suite: {name}")
    }
}
