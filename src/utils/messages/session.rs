use std::fmt;

pub mod errors {
    use super::fmt;

    pub fn invalid(reason: impl fmt::Display) -> String {
        format!("invalid session: {reason}")
    }

    pub fn storage(reason: impl fmt::Display) -> String {
        format!("storage error: {reason}")
    }

    pub const INVALID_ID: &str = "invalid session id";
}

pub mod store {
    pub const LOCKED: &str = "session store locked";
}
