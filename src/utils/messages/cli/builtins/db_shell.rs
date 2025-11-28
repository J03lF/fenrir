pub mod command {
    pub const NAME: &str = "db-shell";
    pub const DESCRIPTION: &str = "Öffnet die Datenbank-Subshell";
    pub const USAGE: &str = "db-shell";
    pub const DETAIL_SWITCH: &str = r"\c <engine> – Engine wechseln";
    pub const DETAIL_TABLES: &str = r"\d [table] – Tabellen auflisten oder Schema anzeigen";
    pub const DETAIL_PING: &str = r"\ping – Verbindung testen";
    pub const DETAIL_EXIT: &str = "exit / \\q – Subshell verlassen";
    pub const DISABLED_NOTE: &str =
        "DB-Shell ist derzeit deaktiviert. Nutze 'start service db-shell', um sie wieder zu aktivieren.";
    pub const STATUS_NOTE_CLI: &str = "DB-Shell (CLI) aktiv";
    pub const STATUS_NOTE_SSH: &str = "DB-Shell (SSH) aktiv";

    pub fn engines_line(engines: &[String]) -> String {
        if engines.is_empty() {
            "keine konfigurierten Engines".to_string()
        } else {
            format!("verfügbar: {}", engines.join(", "))
        }
    }

    pub fn starting_cli(default_engine: &str, engines_line: &str) -> String {
        format!("Starte DB-Shell (Standard: {default_engine}) – {engines_line}")
    }

    pub fn starting_ssh(default_engine: &str, engines_line: &str) -> String {
        format!("Wechsle in DB-Shell (Standard: {default_engine}) – {engines_line}")
    }
}

pub mod shell {
    pub fn active(engine: &str) -> String {
        format!("DB-Shell aktiv. Aktueller Engine: {engine}")
    }

    pub const HELP_HINT: &str = "Nutze 'help' für Übersicht der Befehle.";
    pub const EXIT_MESSAGE: &str = "DB-Shell beendet";
    pub const INPUT_ERROR_PREFIX: &str = "Eingabefehler: ";
    pub const ERROR_PREFIX: &str = "Fehler: ";
}

pub mod process {
    pub const PING_OK: &str = "Ping erfolgreich";
    pub const TABLE_NAME_HINT: &str = "Bitte Tabellenname angeben, z. B. \\d public.tickets";
    pub const SWITCH_PROMPT: &str = "Wechsel mit \\c <engine>";

    pub fn current_engine(engine: &str) -> String {
        format!("Aktueller Engine: {engine}")
    }

    pub fn available_engines(list: &[String]) -> String {
        if list.is_empty() {
            "Verfügbare Engines: -".to_string()
        } else {
            format!("Verfügbare Engines: {}", list.join(", "))
        }
    }

    pub fn engine_switched(engine: &str) -> String {
        format!("Engine gewechselt zu {engine}")
    }

    pub const META_HEADER: &str = "Meta-Befehle:";
    pub const SQL_HEADER: &str = "SQL-Befehle:";
    pub const META_SWITCH: &str = "  \\c <engine>    – Engine wechseln";
    pub const META_TABLES: &str = "  \\d [table]    – Tabellen auflisten oder Schema anzeigen";
    pub const META_PING: &str = "  \\ping         – Verbindung testen";
    pub const META_EXIT: &str = "  exit / \\q    – DB-Shell verlassen";

    pub fn meta_engine_line(engine: &str, options: &str) -> String {
        format!("Aktueller Engine: {engine} (verfügbar: {options})")
    }

    pub fn guard_instruction(warning: &str) -> String {
        format!("  {} (DROP/DELETE/ALTER/TRUNCATE)", warning)
    }
}

pub mod render {
    pub const NO_RESULTS: &str = "(keine Rückgabe)";
    pub const NO_TABLES: &str = "Keine Tabellen gefunden";
    pub const UNKNOWN_SCHEMA: &str = "<unbekannt>";
    pub const NO_COLUMNS: &str = "(keine Spalten)";
    pub const NULLABLE: &str = "NULL";
    pub const NOT_NULL: &str = "NOT NULL";

    pub fn rows_affected(rows: u64) -> String {
        format!("{rows} Zeile(n) betroffen")
    }

    pub fn schema_heading(schema: &str, table: &str) -> String {
        format!("Schema für {schema}.{table}:")
    }

    pub fn result_rows(count: usize) -> String {
        format!("({count} Zeile(n))")
    }
}

pub mod errors {
    pub fn engine_not_configured(engine: &str) -> String {
        format!("Engine {engine} ist nicht konfiguriert")
    }

    pub const CONNECTION: &str = "Verbindungsfehler";
}
