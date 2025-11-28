pub mod command {
    pub const NAME: &str = "user";
    pub const DESCRIPTION: &str = "Verwaltet Control-Plane-Identitäten";
    pub const USAGE: &str = "user <list|issue> …";
    pub const DETAILS: &[&str] = &[
        "user list – zeigt registrierte Identity-Benutzer",
        "user issue <user> [--role <admin|operator|viewer>] [--display-name <name>] – stellt einen Control-Plane-Token aus",
        "user tokens <user> – zeigt vergebene Token (Fingerprints, Laufzeiten) für einen Benutzer",
    ];
    pub const ACTION_COMPLETIONS: &[&str] = &["list", "issue", "tokens"];
    pub const USAGE_HINT: &str = "Nutzung: user <list|issue> …";

    pub fn unknown_action(action: &str) -> String {
        format!("Unbekannte Aktion '{action}'. Verfügbare Aktionen: list, issue, tokens")
    }
}

pub mod identity {
    pub const SERVICE_UNAVAILABLE: &str = "Identity-Service ist nicht verfügbar";
}

pub mod issue {
    pub fn usage(role_hint: &str) -> String {
        format!("Nutzung: user issue <user> [--role <{role_hint}>] [--display-name <name>]")
    }

    pub fn role_missing_value(role_hint: &str) -> String {
        format!("--role erwartet einen Wert ({role_hint})")
    }

    pub const DISPLAY_NAME_MISSING: &str = "--display-name erwartet einen Wert";

    pub fn unknown_option(flag: &str) -> String {
        format!("Unbekannte Option '{flag}'")
    }

    pub fn token_summary(user: &str, role: &str) -> String {
        format!("Token für '{user}' ({role})")
    }

    pub fn token_id(token_id: &str) -> String {
        format!("Token-ID: {token_id}")
    }

    pub fn fingerprint(fingerprint: &str) -> String {
        format!("Fingerprint: {fingerprint}")
    }

    pub fn valid_until(when: &str) -> String {
        format!("Gültig bis: {when}")
    }

    pub const TOKEN_BODY_HINT: &str = "Token (kopieren & sicher speichern):";
}

pub mod list {
    pub const NO_USERS: &str = "Keine Identity-Benutzer registriert.";
    pub const HEADERS: &[&str] = &["User", "Rolle", "Tokens", "Zuletzt Ausgestellt"];
    pub const EMPTY_TIMESTAMP: &str = "-";
}

pub mod tokens {
    pub const USAGE: &str = "Nutzung: user tokens <user>";
    pub fn none_for_user(user: &str) -> String {
        format!("Keine Token für '{user}' vorhanden.")
    }

    pub fn user_not_found(user: &str) -> String {
        format!("Identity-Benutzer '{user}' nicht gefunden.")
    }

    pub const HEADERS: &[&str] = &[
        "Token-ID",
        "Fingerprint",
        "Ausgestellt",
        "Gültig bis",
        "Key-ID",
    ];
}

pub mod roles {
    pub fn unknown_role(value: &str, allowed: &str) -> String {
        format!("Unbekannte Rolle '{value}'. Erlaubt: {allowed}")
    }
}
