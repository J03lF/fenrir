use super::user;
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionKind, ShellEnvironment,
};
use std::io::{self, Write};

const LOCK_RESOURCE_OPTIONS: &[&str] = &["user", "users"];

const LOCK_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(LOCK_RESOURCE_OPTIONS));
const LOCK_USER_ARGUMENT: CommandArgument = CommandArgument::required("username");

const LOCK_ARGUMENTS: &[CommandArgument] = &[LOCK_RESOURCE_ARGUMENT, LOCK_USER_ARGUMENT];
const LOCK_SHAPE: CommandShape = CommandShape::new("lock", &[], LOCK_ARGUMENTS, &[]);

const LOCK_DETAILS: &[&str] = &["lock user <username> – sperrt einen Benutzer"];

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "lock",
        "Sperrt einen Benutzer",
        "lock user <username>",
        LOCK_DETAILS,
        handle_lock,
        LOCK_SHAPE,
    )
}

fn handle_lock(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "Nutzung: lock user <username>")?;
        return Ok(CommandOutcome::Continue);
    };

    if !(resource.eq_ignore_ascii_case("user") || resource.eq_ignore_ascii_case("users")) {
        writeln!(out, "unbekannte Ressource: {resource}")?;
        writeln!(out, "verfügbar: lock user <username>")?;
        return Ok(CommandOutcome::Continue);
    }

    user::set_lock_state(&deps.services.user, tail, true, out)?;
    Ok(CommandOutcome::Continue)
}
