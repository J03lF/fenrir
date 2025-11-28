use crate::cli::commands::registry::{CommandArgument, CommandEntry, CommandShape, CompletionKind};
use crate::utils::messages::cli::builtins::show::command as show_command_messages;

use super::completion::complete_show_targets;
use super::handler::handle_show;

const SHOW_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(
        show_command_messages::RESOURCE_COMPLETIONS,
    ));
const SHOW_TARGET_ARGUMENT: CommandArgument = CommandArgument::required("target")
    .with_completion(CompletionKind::Dynamic(complete_show_targets));

const SHOW_ARGUMENTS: &[CommandArgument] = &[SHOW_RESOURCE_ARGUMENT, SHOW_TARGET_ARGUMENT];
const SHOW_SHAPE: CommandShape = CommandShape::new("show", &[], SHOW_ARGUMENTS, &[]);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        show_command_messages::NAME,
        show_command_messages::DESCRIPTION,
        show_command_messages::USAGE,
        show_command_messages::DETAILS,
        handle_show,
        SHOW_SHAPE,
    )
}
