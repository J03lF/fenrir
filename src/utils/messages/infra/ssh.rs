use std::fmt;

pub fn db_shell_disabled_return() -> &'static str {
    "DB shell is disabled. Returning to the main shell."
}

pub fn db_shell_disabled_return_short() -> &'static str {
    "DB shell was disabled. Returning to the main shell."
}

pub fn db_shell_disabled_hint() -> &'static str {
    "DB shell is disabled. Use 'start service db-shell'."
}

pub fn db_shell_not_initialized() -> &'static str {
    "DB shell not initialized"
}

pub fn db_shell_active(engine: impl fmt::Display) -> String {
    format!("DB shell active. Current engine: {engine}")
}

pub fn db_shell_engines_list(list: &str) -> String {
    format!("Available engines: {list}")
}

pub fn db_shell_force_hint(warning: &str) -> String {
    format!("Hint: {warning}")
}

pub fn db_shell_start_failed(err: impl fmt::Display) -> String {
    format!("Failed to start DB shell: {err}")
}

pub fn command_execution_failed() -> &'static str {
    "command execution failed"
}

pub fn command_execution_error(err: impl fmt::Display) -> String {
    format!("command execution error: {err}")
}

pub fn completion_hidden_hint(hidden: usize) -> String {
    let suffix = if hidden == 1 { "" } else { "s" };
    format!("... {hidden} more suggestion{suffix} hidden (press TAB to cycle).")
}

pub fn service_failure_note(err: impl fmt::Display) -> String {
    format!("error: {err}")
}
