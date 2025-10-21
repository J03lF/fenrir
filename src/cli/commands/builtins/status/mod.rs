use super::ticket;
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionKind, ShellEnvironment,
};
use std::io::{self, Write};

const STATUS_RESOURCE_OPTIONS: &[&str] = &["ticket", "tickets"];

const STATUS_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(STATUS_RESOURCE_OPTIONS));
const STATUS_TICKET_ARGUMENT: CommandArgument = CommandArgument::required("ticket-id");
const STATUS_VALUE_ARGUMENT: CommandArgument = CommandArgument::required("status")
    .with_completion(CompletionKind::Static(ticket::TICKET_STATUS_VALUES));

const STATUS_ARGUMENTS: &[CommandArgument] = &[
    STATUS_RESOURCE_ARGUMENT,
    STATUS_TICKET_ARGUMENT,
    STATUS_VALUE_ARGUMENT,
];
const STATUS_SHAPE: CommandShape = CommandShape::new("status", &[], STATUS_ARGUMENTS, &[]);

const STATUS_DETAILS: &[&str] = &["status ticket <ticket-id> <status> – setzt den Ticketstatus"];

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "status",
        "Setzt den Status eines Tickets",
        "status ticket <ticket-id> <status>",
        STATUS_DETAILS,
        handle_status,
        STATUS_SHAPE,
    )
}

fn handle_status(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "Nutzung: status ticket <ticket-id> <status>")?;
        return Ok(CommandOutcome::Continue);
    };

    if !(resource.eq_ignore_ascii_case("ticket") || resource.eq_ignore_ascii_case("tickets")) {
        writeln!(out, "unbekannte Ressource: {resource}")?;
        writeln!(out, "verfügbar: status ticket <ticket-id> <status>")?;
        return Ok(CommandOutcome::Continue);
    }

    if tail.len() < 2 {
        writeln!(out, "Nutzung: status ticket <ticket-id> <status>")?;
        return Ok(CommandOutcome::Continue);
    }

    ticket::update_status(&deps.services.ticket, tail, out)?;
    Ok(CommandOutcome::Continue)
}
