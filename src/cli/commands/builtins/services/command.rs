use super::actions::{handle_list, handle_restart, handle_start, handle_stop};
use super::metadata::{LIST_SHAPE, RESTART_ACTION, START_ACTION, STOP_ACTION};
use crate::cli::commands::registry::CommandEntry;
use crate::utils::messages::cli::builtins::services::list_command;

pub fn start_command() -> CommandEntry {
    START_ACTION.as_entry(handle_start)
}

pub fn stop_command() -> CommandEntry {
    STOP_ACTION.as_entry(handle_stop)
}

pub fn restart_command() -> CommandEntry {
    RESTART_ACTION.as_entry(handle_restart)
}

pub fn list_command() -> CommandEntry {
    CommandEntry::with_shape(
        "list",
        list_command::DESCRIPTION,
        list_command::SYNOPSIS,
        list_command::DETAILS,
        handle_list,
        LIST_SHAPE,
    )
}
