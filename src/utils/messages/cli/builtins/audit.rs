use std::fmt;

pub mod command {
    pub const NAME: &str = "audit";
    pub const DESCRIPTION: &str = "Listet aktuelle Audit-Events (nur lesend)";
    pub const USAGE: &str =
        "audit [--limit <n>] [--action <code>] [--outcome <status>] [--actor <id>]";
    pub const DETAILS: &[&str] = &[
        "audit                      – zeigt die letzten 20 Audit-Einträge",
        "audit --limit <n>          – begrenzt die Anzahl der Einträge",
        "audit --action <code>      – filtert nach Aktions-Code",
        "audit --outcome <status>   – filtert nach Ergebnis (success|failure|denied)",
        "audit --actor system       – nur Systemereignisse anzeigen",
    ];
}

pub mod responses {
    use super::*;

    pub const DISABLED: &str =
        "Audit-Logging ist laut Konfiguration deaktiviert. Aktivieren via [audit.enabled].";
    pub const MISSING_LIMIT_VALUE: &str = "Fehlender Wert nach --limit";
    pub const INVALID_LIMIT: &str = "Ungültiger --limit Wert. Erlaubt: positive Ganzzahlen";
    pub fn unknown_parameter(param: &str) -> String {
        format!("Unbekannter Parameter: {param}")
    }
    pub const ZERO_LIMIT_WARNING: &str = "Limit 0 liefert keine Ergebnisse.";
    pub fn load_failed(err: impl fmt::Display) -> String {
        format!("Konnte Audit-Events nicht laden: {err}")
    }
    pub const NONE_AVAILABLE: &str = "Keine Audit-Ereignisse vorhanden.";
    pub fn header(limit: usize) -> String {
        format!("Audit-Events (neueste zuerst, max. {limit}):")
    }
    pub const FILTER_NONE: &str = "Keine Audit-Ereignisse entsprechen den gesetzten Filtern.";
}

pub mod render {
    pub const INVALID_TIMESTAMP: &str = "<invalid>";
    pub const META_PREFIX: &str = "  meta: ";
    pub const REDACTIONS_PREFIX: &str = "  redactions: ";
    pub const REDACTED_VALUE: &str = "<redacted>";

    pub fn event_line(ts: &str, actor: &str, action: &str, target: &str, outcome: &str) -> String {
        format!("{ts} | actor={actor} | action={action} | target={target} | outcome={outcome}")
    }

    pub fn format_meta_entry(key: &str, value: &str) -> String {
        format!("{key}={value}")
    }

    pub fn format_redacted_entry(key: &str) -> String {
        format!("{key}={}", REDACTED_VALUE)
    }

    pub fn actor_user_with_role(role: &str) -> String {
        format!("user(role={role})")
    }

    pub fn actor_user_with_id(user_id: &str, role: &str) -> String {
        format!("user(id={user_id}, role={role})")
    }
}
