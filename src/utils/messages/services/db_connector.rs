pub mod logs {
    pub const REQUEST_OK: &str = "db connector request processed";
    pub const REQUEST_FAILED: &str = "db connector request failed";
}

pub mod errors {
    pub const STATEMENT_EMPTY: &str = "statement must not be empty";

    pub fn statement_too_long(limit: usize) -> String {
        format!("statement exceeds maximum size ({limit} bytes)")
    }

    pub fn actors_service_only() -> &'static str {
        "db connector accepts service tokens only"
    }

    pub fn missing_scope(scope: &str) -> String {
        format!("connector access requires scope '{scope}'")
    }

    pub fn tenant_policy_requires_prepared() -> &'static str {
        "tenant policies require prepared statements"
    }

    pub fn tenant_param_missing(name: &str) -> String {
        format!("tenant binding parameter '{name}' is missing")
    }

    pub fn tenant_param_mismatch(name: &str) -> String {
        format!("tenant binding parameter '{name}' does not match the caller tenant")
    }
}

pub mod responses {
    use std::fmt;

    pub fn invalid_payload(err: impl fmt::Display) -> String {
        format!("invalid request payload: {err}")
    }

    pub const TOO_LARGE: &str = "request payload too large";
}
