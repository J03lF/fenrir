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

pub mod connector {
    use super::*;

    pub const ACCEPT_FAILED: &str = "db connector accept loop failed";
    pub const STREAM_FAILED: &str = "db connector stream handling failed";
    pub const TASK_STARTED: &str = "db connector task started";
    pub const TASK_EXITED: &str = "db connector task exited";
    pub const IPC_READY: &str = "db connector ipc socket ready";
    pub const TCP_READY: &str = "db connector tcp listener ready";
    pub const TASK_IPC: &str = "db-connector-ipc";
    pub const TASK_TCP: &str = "db-connector-tcp";

    pub fn addr_unknown() -> &'static str {
        "db connector listener has no local addr"
    }

    pub fn request_invalid(err: impl fmt::Display) -> String {
        format!("invalid connector request: {err}")
    }

    pub fn request_too_large() -> &'static str {
        "connector request exceeds maximum size"
    }
}
