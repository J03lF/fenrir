use std::fmt;

pub mod postgres {
    use super::*;

    pub fn uri_empty() -> &'static str {
        "postgres uri must not be empty"
    }

    pub fn invalid_config(err: impl fmt::Display) -> String {
        format!("invalid postgres config: {err}")
    }

    pub fn connection_timeout() -> &'static str {
        "connection timeout"
    }

    pub fn query_timeout() -> &'static str {
        "query timeout"
    }

    pub fn empty_ping_response() -> &'static str {
        "empty response to SELECT 1"
    }

    pub fn statement_empty() -> &'static str {
        "statement must not be empty"
    }

    pub fn table_not_found(schema: &str, table: &str) -> String {
        format!("table {schema}.{table} not found")
    }
}

pub mod manager {
    pub const ADAPTER_UNIMPLEMENTED: &str = "db adapter not implemented yet";
}
