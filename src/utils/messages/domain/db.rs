use std::fmt;

pub mod engine {
    use super::*;

    pub fn not_configured(engine: impl fmt::Display) -> String {
        format!("db engine not configured: {engine}")
    }

    pub fn unknown(value: &str) -> String {
        format!("unknown db engine: {value}")
    }
}

pub mod errors {
    use super::*;

    pub fn connection(message: impl fmt::Display) -> String {
        format!("db connection error: {message}")
    }

    pub fn query(message: impl fmt::Display) -> String {
        format!("db query error: {message}")
    }

    pub fn invalid_input(message: impl fmt::Display) -> String {
        format!("invalid input: {message}")
    }

    pub fn not_implemented(message: impl fmt::Display) -> String {
        format!("function not implemented: {message}")
    }
}
