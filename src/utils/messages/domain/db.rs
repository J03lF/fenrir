use std::fmt;

pub mod engine {
    use super::*;

    pub fn not_configured(engine: impl fmt::Display) -> String {
        format!("DB-Engine nicht konfiguriert: {engine}")
    }

    pub fn unknown(value: &str) -> String {
        format!("unbekannter DB-Typ: {value}")
    }
}

pub mod errors {
    use super::*;

    pub fn connection(message: impl fmt::Display) -> String {
        format!("DB-Verbindungsfehler: {message}")
    }

    pub fn query(message: impl fmt::Display) -> String {
        format!("DB-Abfragefehler: {message}")
    }

    pub fn invalid_input(message: impl fmt::Display) -> String {
        format!("Ungültige Eingabe: {message}")
    }

    pub fn not_implemented(message: impl fmt::Display) -> String {
        format!("Funktion nicht implementiert: {message}")
    }
}
