use std::fmt;

pub fn unknown_service_role(value: impl fmt::Display) -> String {
    format!("unknown service role '{value}'")
}

pub fn invalid_scope(value: impl fmt::Display) -> String {
    format!("invalid service scope '{value}' (expected <domain>:<action>)")
}

pub fn scope_empty() -> &'static str {
    "service scope must not be empty"
}

pub fn scope_too_long(max: usize) -> String {
    format!("service scope exceeds maximum length of {max} characters")
}

pub fn scope_invalid_segment() -> &'static str {
    "service scope segments may only contain lowercase letters, numbers, '-' or '_'"
}
