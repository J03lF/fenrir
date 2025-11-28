use std::fmt;

pub mod postgres {
    use super::*;

    pub fn uri_empty() -> &'static str {
        "postgres uri must not be empty"
    }

    pub fn invalid_config(err: impl fmt::Display) -> String {
        format!("ungültige Postgres-Config: {err}")
    }

    pub fn connection_timeout() -> &'static str {
        "Verbindungs-Timeout"
    }

    pub fn query_timeout() -> &'static str {
        "Query-Timeout"
    }

    pub fn empty_ping_response() -> &'static str {
        "leere Antwort auf SELECT 1"
    }

    pub fn statement_empty() -> &'static str {
        "Statement darf nicht leer sein"
    }

    pub fn table_not_found(schema: &str, table: &str) -> String {
        format!("Tabelle {schema}.{table} nicht gefunden")
    }
}

pub mod manager {
    pub const ADAPTER_UNIMPLEMENTED: &str = "DB-Adapter noch nicht implementiert";
}
