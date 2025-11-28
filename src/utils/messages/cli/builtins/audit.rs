use std::fmt;

pub mod command {
    pub const NAME: &str = "audit";
    pub const DESCRIPTION: &str = "Lists recent audit events (read-only)";
    pub const USAGE: &str =
        "audit [--limit <n>] [--action <code>] [--outcome <status>] [--actor <id>]";
    pub const DETAILS: &[&str] = &[
        "audit                      – shows the last 20 audit entries",
        "audit --limit <n>          – limits the number of entries",
        "audit --action <code>      – filters by action code",
        "audit --outcome <status>   – filters by outcome (success|failure|denied)",
        "audit --actor system       – only show system events",
    ];
}

pub mod responses {
    use super::*;

    pub const DISABLED: &str =
        "Audit logging is disabled in the configuration. Enable via [audit.enabled].";
    pub const MISSING_LIMIT_VALUE: &str = "missing value after --limit";
    pub const INVALID_LIMIT: &str = "invalid --limit value. Allowed: positive integers";
    pub fn unknown_parameter(param: &str) -> String {
        format!("unknown parameter: {param}")
    }
    pub const ZERO_LIMIT_WARNING: &str = "limit 0 returns no results.";
    pub fn load_failed(err: impl fmt::Display) -> String {
        format!("failed to load audit events: {err}")
    }
    pub const NONE_AVAILABLE: &str = "No audit events available.";
    pub fn header(limit: usize) -> String {
        format!("Audit events (newest first, max. {limit}):")
    }
    pub const FILTER_NONE: &str = "No audit events match the configured filters.";
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
