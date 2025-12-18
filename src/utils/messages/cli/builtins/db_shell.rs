pub mod command {
    pub const NAME: &str = "db-shell";
    pub const DESCRIPTION: &str = "Opens the database subshell";
    pub const USAGE: &str = "db-shell";
    pub const DETAIL_SWITCH: &str = r"\c <engine> – switch engines";
    pub const DETAIL_TABLES: &str = r"\d [table] – list tables or show schema";
    pub const DETAIL_PING: &str = r"\ping – test the connection";
    pub const DETAIL_EXIT: &str = "exit / \\q – leave the subshell";
    pub const DISABLED_NOTE: &str =
        "The DB shell is currently disabled. Use 'start service db-shell' to enable it.";
    pub const STATUS_NOTE_CLI: &str = "DB shell (CLI) active";
    pub const STATUS_NOTE_SSH: &str = "DB shell (SSH) active";

    pub fn engines_line(engines: &[String]) -> String {
        if engines.is_empty() {
            "no engines configured".to_string()
        } else {
            format!("available: {}", engines.join(", "))
        }
    }

    pub fn starting_cli(default_engine: &str, engines_line: &str) -> String {
        format!("Starting DB shell (default: {default_engine}) – {engines_line}")
    }

    pub fn starting_ssh(default_engine: &str, engines_line: &str) -> String {
        format!("Switching into DB shell (default: {default_engine}) – {engines_line}")
    }
}

pub mod shell {
    pub fn active(engine: &str) -> String {
        format!("DB shell active. Current engine: {engine}")
    }

    pub const HELP_HINT: &str = "Use 'help' for a command overview.";
    pub const MULTILINE_HINT: &str = "Multi-line: end SQL with ';' to execute. Ctrl+C to cancel.";
    pub const BUFFER_CLEARED: &str = "(buffer cleared)";
    pub const EXIT_MESSAGE: &str = "DB shell exited";
    pub const INPUT_ERROR_PREFIX: &str = "Input error: ";
    pub const ERROR_PREFIX: &str = "Error: ";
}

pub mod process {
    pub const PING_OK: &str = "Ping successful";
    pub const TABLE_NAME_HINT: &str = "Please provide a table name, e.g. \\d public.tickets";
    pub const SWITCH_PROMPT: &str = "Switch with \\c <engine>";

    pub fn current_engine(engine: &str) -> String {
        format!("Current engine: {engine}")
    }

    pub fn available_engines(list: &[String]) -> String {
        if list.is_empty() {
            "Available engines: -".to_string()
        } else {
            format!("Available engines: {}", list.join(", "))
        }
    }

    pub fn engine_switched(engine: &str) -> String {
        format!("Engine switched to {engine}")
    }

    pub const META_HEADER: &str = "Meta commands:";
    pub const SQL_HEADER: &str = "SQL commands:";
    pub const META_SWITCH: &str = "  \\c <engine>    – switch engines";
    pub const META_TABLES: &str = "  \\d [table]     – list tables or show schema";
    pub const META_PING: &str = "  \\ping          – test the connection";
    pub const META_REFRESH: &str = "  \\refresh       – reload table completion";
    pub const META_EXIT: &str = "  exit / \\q      – leave the DB shell";

    pub fn meta_engine_line(engine: &str, options: &str) -> String {
        format!("Current engine: {engine} (available: {options})")
    }

    pub fn guard_instruction(warning: &str) -> String {
        format!("  {} (DROP/DELETE/ALTER/TRUNCATE)", warning)
    }
}

pub mod render {
    pub const NO_RESULTS: &str = "(no rows returned)";
    pub const NO_TABLES: &str = "No tables found";
    pub const UNKNOWN_SCHEMA: &str = "<unknown>";
    pub const NO_COLUMNS: &str = "(no columns)";
    pub const NULLABLE: &str = "NULL";
    pub const NOT_NULL: &str = "NOT NULL";

    pub fn rows_affected(rows: u64) -> String {
        format!("{rows} row(s) affected")
    }

    pub fn schema_heading(schema: &str, table: &str) -> String {
        format!("Schema for {schema}.{table}:")
    }

    pub fn result_rows(count: usize) -> String {
        format!("({count} row(s))")
    }
}

pub mod errors {
    pub fn engine_not_configured(engine: &str) -> String {
        format!("Engine {engine} is not configured")
    }

    pub const CONNECTION: &str = "Connection error";
}
