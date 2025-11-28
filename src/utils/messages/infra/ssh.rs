use std::fmt;

pub fn db_shell_disabled_return() -> &'static str {
    "DB-Shell ist deaktiviert. Zurück zur Hauptshell."
}

pub fn db_shell_disabled_return_short() -> &'static str {
    "DB-Shell wurde deaktiviert. Rückkehr zur Hauptshell."
}

pub fn db_shell_disabled_hint() -> &'static str {
    "DB-Shell ist deaktiviert. Nutze 'start service db-shell'."
}

pub fn db_shell_not_initialized() -> &'static str {
    "DB-Shell nicht initialisiert"
}

pub fn db_shell_active(engine: impl fmt::Display) -> String {
    format!("DB-Shell aktiv. Aktueller Engine: {engine}")
}

pub fn db_shell_engines_list(list: &str) -> String {
    format!("Verfügbare Engines: {list}")
}

pub fn db_shell_force_hint(warning: &str) -> String {
    format!("Hinweis: {warning}")
}

pub fn db_shell_start_failed(err: impl fmt::Display) -> String {
    format!("DB-Shell konnte nicht gestartet werden: {err}")
}

pub fn command_execution_failed() -> &'static str {
    "Fehler bei der Befehlsausführung"
}

pub fn command_execution_error(err: impl fmt::Display) -> String {
    format!("Fehler bei der Befehlsausführung: {err}")
}

pub fn completion_hidden_hint(hidden: usize) -> String {
    let suffix = if hidden == 1 { "" } else { "s" };
    format!("... {hidden} more suggestion{suffix} hidden (press TAB to cycle).")
}

pub fn service_failure_note(err: impl fmt::Display) -> String {
    format!("Fehler: {err}")
}
