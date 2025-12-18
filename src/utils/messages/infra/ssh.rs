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

// Password setup messages
pub fn password_setup_welcome() -> &'static str {
    "\x1b[38;5;81m\
     ┌─────────────────────────────────────────────────────────────────┐\n\
     │                                                                 │\n\
     │   \x1b[1;97mACCOUNT SETUP\x1b[0m\x1b[38;5;81m                                                │\n\
     │                                                                 │\n\
     │   \x1b[38;5;250mWelcome to Fenrir. This is your first login.\x1b[38;5;81m                 │\n\
     │   \x1b[38;5;250mPlease create a password to secure your account.\x1b[38;5;81m              │\n\
     │                                                                 │\n\
     └─────────────────────────────────────────────────────────────────┘\x1b[0m"
}

pub fn password_setup_enter_prompt() -> &'static str {
    "\x1b[38;5;81m   >\x1b[0m New password:     "
}

pub fn password_setup_confirm_prompt() -> &'static str {
    "\x1b[38;5;81m   >\x1b[0m Confirm password: "
}

pub fn password_validation_failed() -> &'static str {
    "\x1b[38;5;203m   [!]\x1b[0m Password requirements not met:"
}

pub fn password_mismatch() -> &'static str {
    "\x1b[38;5;203m   [!]\x1b[0m Passwords do not match. Please try again.\n"
}

pub fn password_setup_success() -> &'static str {
    "\n\x1b[38;5;81m   [ok]\x1b[0m \x1b[38;5;250mPassword configured successfully.\x1b[0m\n"
}

pub fn password_setup_reconnect() -> &'static str {
    "   Please reconnect with your new credentials."
}

pub fn password_setup_cancelled() -> &'static str {
    "\x1b[38;5;203m   [x]\x1b[0m Setup cancelled."
}

pub fn password_setup_error(err: impl fmt::Display) -> String {
    format!("\x1b[38;5;203m   [!]\x1b[0m Setup failed: {err}")
}
