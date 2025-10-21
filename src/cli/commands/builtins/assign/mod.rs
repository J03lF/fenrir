use super::ticket;
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionContext, CompletionKind, ShellEnvironment,
};
use std::io::{self, Write};

const ASSIGN_RESOURCE_OPTIONS: &[&str] = &["ticket", "tickets"];

const ASSIGN_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(ASSIGN_RESOURCE_OPTIONS));
const ASSIGN_TICKET_ARGUMENT: CommandArgument = CommandArgument::required("ticket-id");
const ASSIGN_TARGET_ARGUMENT: CommandArgument = CommandArgument::required("assignee")
    .with_completion(CompletionKind::Dynamic(complete_assign_targets));

const ASSIGN_ARGUMENTS: &[CommandArgument] = &[
    ASSIGN_RESOURCE_ARGUMENT,
    ASSIGN_TICKET_ARGUMENT,
    ASSIGN_TARGET_ARGUMENT,
];
const ASSIGN_SHAPE: CommandShape = CommandShape::new("assign", &[], ASSIGN_ARGUMENTS, &[]);

const ASSIGN_DETAILS: &[&str] =
    &["assign ticket <ticket-id> <username|none> – weist ein Ticket zu"];

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "assign",
        "Weist Tickets einem Benutzer zu oder gibt sie frei",
        "assign ticket <ticket-id> <username|none>",
        ASSIGN_DETAILS,
        handle_assign,
        ASSIGN_SHAPE,
    )
}

fn handle_assign(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "Nutzung: assign ticket <ticket-id> <username|none>")?;
        return Ok(CommandOutcome::Continue);
    };

    if !(resource.eq_ignore_ascii_case("ticket") || resource.eq_ignore_ascii_case("tickets")) {
        writeln!(out, "unbekannte Ressource: {resource}")?;
        writeln!(out, "verfügbar: assign ticket <ticket-id> <username|none>")?;
        return Ok(CommandOutcome::Continue);
    }

    if tail.len() < 2 {
        writeln!(out, "Nutzung: assign ticket <ticket-id> <username|none>")?;
        return Ok(CommandOutcome::Continue);
    }

    ticket::update_assignment(&deps.services.ticket, &deps.services.user, tail, out)?;
    Ok(CommandOutcome::Continue)
}

fn complete_assign_targets(_deps: &CliDependencies, _ctx: &CompletionContext<'_>) -> Vec<String> {
    ticket::TICKET_ASSIGN_SPECIAL
        .iter()
        .map(|value| (*value).to_string())
        .collect()
}
