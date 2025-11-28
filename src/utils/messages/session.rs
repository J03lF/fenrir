use std::fmt;

pub mod errors {
    use super::fmt;

    pub fn invalid(reason: impl fmt::Display) -> String {
        format!("ungültige Session: {reason}")
    }

    pub fn storage(reason: impl fmt::Display) -> String {
        format!("Storage-Fehler: {reason}")
    }

    pub const INVALID_ID: &str = "ungültige Session-ID";
}

pub mod store {
    pub const LOCKED: &str = "Session-Store gesperrt";
}
