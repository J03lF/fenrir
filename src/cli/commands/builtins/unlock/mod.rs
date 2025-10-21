use super::user;
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionKind, ShellEnvironment,
};
use std::io::{self, Write};

const UNLOCK_RESOURCE_OPTIONS: &[&str] = &["user", "users"];

const UNLOCK_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(UNLOCK_RESOURCE_OPTIONS));
const UNLOCK_USER_ARGUMENT: CommandArgument = CommandArgument::required("username");

const UNLOCK_ARGUMENTS: &[CommandArgument] = &[UNLOCK_RESOURCE_ARGUMENT, UNLOCK_USER_ARGUMENT];
const UNLOCK_SHAPE: CommandShape = CommandShape::new("unlock", &[], UNLOCK_ARGUMENTS, &[]);

const UNLOCK_DETAILS: &[&str] = &["unlock user <username> – entsperrt einen Benutzer"];

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "unlock",
        "Entsperrt einen Benutzer",
        "unlock user <username>",
        UNLOCK_DETAILS,
        handle_unlock,
        UNLOCK_SHAPE,
    )
}

fn handle_unlock(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "Nutzung: unlock user <username>")?;
        return Ok(CommandOutcome::Continue);
    };

    if !(resource.eq_ignore_ascii_case("user") || resource.eq_ignore_ascii_case("users")) {
        writeln!(out, "unbekannte Ressource: {resource}")?;
        writeln!(out, "verfügbar: unlock user <username>")?;
        return Ok(CommandOutcome::Continue);
    }

    user::set_lock_state(&deps.services.user, tail, false, out)?;
    Ok(CommandOutcome::Continue)
}
