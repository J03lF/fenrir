use crate::cli::commands::registry::{CommandEntry, CommandShape};
use crate::utils::messages::cli::builtins::clear::command as clear_messages;

use super::handler::handle;

const CLEAR_SHAPE: CommandShape = CommandShape::basic("clear");

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        clear_messages::NAME,
        clear_messages::DESCRIPTION,
        clear_messages::USAGE,
        clear_messages::DETAILS,
        handle,
        CLEAR_SHAPE,
    )
}
