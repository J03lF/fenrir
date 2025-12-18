use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionKind, ShellEnvironment,
};
use crate::utils::messages::cli::builtins::user::command as user_command_messages;
use std::io::{self, Write};

use super::issue::handle_issue;
use super::list::handle_list;
use super::password::handle_password;
use super::tokens::handle_tokens;

const USER_ARGUMENTS: &[CommandArgument] = &[CommandArgument::required("action")
    .with_completion(CompletionKind::Static(
        user_command_messages::ACTION_COMPLETIONS,
    ))
    .variadic()];

const USER_SHAPE: CommandShape = CommandShape::new("user", &[], USER_ARGUMENTS, &[]);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        user_command_messages::NAME,
        user_command_messages::DESCRIPTION,
        user_command_messages::USAGE,
        user_command_messages::DETAILS,
        handle_user,
        USER_SHAPE,
    )
}

fn handle_user(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((action, rest)) = args.split_first() else {
        writeln!(out, "{}", user_command_messages::USAGE_HINT)?;
        return Ok(CommandOutcome::Continue);
    };

    match action.to_ascii_lowercase().as_str() {
        "list" => handle_list(deps, out),
        "issue" => handle_issue(deps, rest, out),
        "tokens" => handle_tokens(deps, rest, out),
        "password" => handle_password(deps, rest, out),
        other => {
            writeln!(out, "{}", user_command_messages::unknown_action(other))?;
            Ok(CommandOutcome::Continue)
        }
    }
}
