use std::io::{self, Write};
use std::str::FromStr;

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CommandSubcommand, CompletionKind, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::domain::ticket::{TicketFilter, TicketId, TicketPriority, TicketStatus};
use crate::domain::user::{UserFilter, UserId};
use crate::services::ticket::CreateTicketCommand;
use crate::services::{TicketService, UserService};
use crate::utils;
use tracing::{info, warn};

const DETAILS: &[&str] = &[
    "list [--status <s1,s2>] [--reporter <user>] [--assignee <user>] [--tag <tag>] [--search <text>]",
    "show <ticket-id>",
    "create --title <titel> --description <text> --priority <prio> --reporter <user> [--assignee <user>] [--tag <tag> ...]",
    "assign <ticket-id> <username|none>",
    "status <ticket-id> <status>",
];

pub(crate) const TICKET_LIST_OPTIONS: &[&str] =
    &["--status", "--reporter", "--assignee", "--tag", "--search"];
pub(crate) const TICKET_CREATE_OPTIONS: &[&str] = &[
    "--title",
    "--description",
    "--priority",
    "--reporter",
    "--assignee",
    "--tag",
];
pub(crate) const TICKET_STATUS_VALUES: &[&str] = &["open", "in_progress", "resolved", "closed"];
pub(crate) const TICKET_ASSIGN_SPECIAL: &[&str] = &["none"];

const TICKET_ID_ARGUMENT: CommandArgument = CommandArgument::required("ticket-id");
const TICKET_LIST_ARGUMENT: CommandArgument = CommandArgument::optional("option")
    .with_completion(CompletionKind::Static(TICKET_LIST_OPTIONS))
    .variadic();
const TICKET_CREATE_ARGUMENT: CommandArgument = CommandArgument::optional("option")
    .with_completion(CompletionKind::Static(TICKET_CREATE_OPTIONS))
    .variadic();
const TICKET_ASSIGN_ARGUMENT: CommandArgument = CommandArgument::required("assignee")
    .with_completion(CompletionKind::Static(TICKET_ASSIGN_SPECIAL));
const TICKET_STATUS_ARGUMENT: CommandArgument = CommandArgument::required("status")
    .with_completion(CompletionKind::Static(TICKET_STATUS_VALUES));

const TICKET_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new("list", &[], &[TICKET_LIST_ARGUMENT], "Tickets auflisten"),
    CommandSubcommand::new("show", &[], &[TICKET_ID_ARGUMENT], "Ticket anzeigen"),
    CommandSubcommand::new("create", &[], &[TICKET_CREATE_ARGUMENT], "Ticket erstellen"),
    CommandSubcommand::new(
        "assign",
        &[],
        &[TICKET_ID_ARGUMENT, TICKET_ASSIGN_ARGUMENT],
        "Ticket zuweisen oder freigeben",
    ),
    CommandSubcommand::new(
        "status",
        &[],
        &[TICKET_ID_ARGUMENT, TICKET_STATUS_ARGUMENT],
        "Ticketstatus ändern",
    ),
];

const TICKET_SHAPE: CommandShape = CommandShape::new("ticket", &[], &[], TICKET_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "ticket",
        "Arbeitet mit Tickets (listen, anlegen, aktualisieren)",
        "ticket <aktion> [optionen]",
        DETAILS,
        handle,
        TICKET_SHAPE,
    )
}

fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let action = args.first().copied().unwrap_or("list");
    info!(command = "ticket", action, "ticket command invoked");
    match action {
        "list" => list_tickets(
            &deps.services.ticket,
            &deps.services.user,
            args.get(1..).unwrap_or_default(),
            out,
        )?,
        "create" => create_ticket(
            &deps.services.ticket,
            &deps.services.user,
            args.get(1..).unwrap_or_default(),
            out,
        )?,
        "assign" => update_assignment(
            &deps.services.ticket,
            &deps.services.user,
            args.get(1..).unwrap_or_default(),
            out,
        )?,
        "status" => update_status(
            &deps.services.ticket,
            args.get(1..).unwrap_or_default(),
            out,
        )?,
        "show" => show_ticket(
            &deps.services.ticket,
            &deps.services.user,
            args.get(1..).unwrap_or_default(),
            out,
        )?,
        other => {
            writeln!(out, "unbekannte Aktion: {other}")?;
            writeln!(out, "verfügbar: ticket <aktion> ...")?;
            for detail in DETAILS {
                writeln!(out, "  - {detail}")?;
            }
            warn!(
                command = "ticket",
                action = other,
                "unknown ticket subcommand"
            );
        }
    }
    Ok(CommandOutcome::Continue)
}

pub(crate) fn list_tickets(
    service: &TicketService,
    users: &UserService,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    let mut filter = TicketFilter::default();
    let mut idx = 0;
    while idx < args.len() {
        match args[idx] {
            "--status" => {
                idx += 1;
                if let Some(arg) = args.get(idx) {
                    let parts = arg.split(',');
                    let mut statuses = Vec::new();
                    for part in parts {
                        match TicketStatus::from_str(part) {
                            Ok(status) => statuses.push(status),
                            Err(err) => {
                                writeln!(out, "Status-Fehler: {err}")?;
                                return Ok(());
                            }
                        }
                    }
                    filter.status = Some(statuses);
                }
            }
            "--reporter" => {
                idx += 1;
                if let Some(username) = args.get(idx) {
                    match users.find_by_username(username) {
                        Ok(Some(user)) => filter.reporter = Some(user.id),
                        Ok(None) => {
                            writeln!(out, "Reporter {username} nicht gefunden")?;
                            return Ok(());
                        }
                        Err(err) => {
                            writeln!(out, "Fehler: {err}")?;
                            return Ok(());
                        }
                    }
                }
            }
            "--assignee" => {
                idx += 1;
                if let Some(username) = args.get(idx) {
                    match users.find_by_username(username) {
                        Ok(Some(user)) => filter.assignee = Some(user.id),
                        Ok(None) => {
                            writeln!(out, "Bearbeiter {username} nicht gefunden")?;
                            return Ok(());
                        }
                        Err(err) => {
                            writeln!(out, "Fehler: {err}")?;
                            return Ok(());
                        }
                    }
                }
            }
            "--tag" => {
                idx += 1;
                if let Some(tag) = args.get(idx) {
                    filter.tags.push(tag.to_ascii_lowercase());
                }
            }
            "--search" => {
                idx += 1;
                if let Some(term) = args.get(idx) {
                    filter.search = Some((*term).to_string());
                }
            }
            other => {
                writeln!(out, "unbekannte Option: {other}")?;
                return Ok(());
            }
        }
        idx += 1;
    }

    let tickets = match service.list(filter) {
        Ok(list) => list,
        Err(err) => {
            writeln!(out, "Fehler bei Ticketliste: {err}")?;
            return Ok(());
        }
    };

    if tickets.is_empty() {
        writeln!(out, "Keine Tickets gefunden")?;
        return Ok(());
    }

    let user_index = build_user_index(users);

    let mut table = Table::new(vec![
        "Ticket-ID".to_string(),
        "Titel".to_string(),
        "Status".to_string(),
        "Prio".to_string(),
        "Reporter".to_string(),
        "Assignee".to_string(),
    ]);
    for ticket in tickets {
        let reporter = user_index
            .get(&ticket.reporter_id)
            .cloned()
            .unwrap_or_else(|| "?".to_string());
        let assignee = ticket
            .assignee_id
            .and_then(|id| user_index.get(&id).cloned())
            .unwrap_or_else(|| "-".to_string());
        table.add_row(vec![
            ticket.id.to_string(),
            truncate(&ticket.title, 24),
            ticket.status.to_string(),
            ticket.priority.to_string(),
            reporter,
            assignee,
        ]);
    }
    table.render(out, "  ")
}

pub(crate) fn create_ticket(
    service: &TicketService,
    users: &UserService,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    let mut title: Option<&str> = None;
    let mut description: Option<&str> = None;
    let mut priority = TicketPriority::Medium;
    let mut reporter: Option<&str> = None;
    let mut assignee: Option<&str> = None;
    let mut tags: Vec<String> = Vec::new();

    let mut idx = 0;
    while idx < args.len() {
        match args[idx] {
            "--title" => {
                idx += 1;
                title = args.get(idx).copied();
            }
            "--description" => {
                idx += 1;
                description = args.get(idx).copied();
            }
            "--priority" => {
                idx += 1;
                if let Some(value) = args.get(idx) {
                    match TicketPriority::from_str(value) {
                        Ok(p) => priority = p,
                        Err(err) => {
                            writeln!(out, "Ungültige Priorität: {err}")?;
                            return Ok(());
                        }
                    }
                }
            }
            "--reporter" => {
                idx += 1;
                reporter = args.get(idx).copied();
            }
            "--assignee" => {
                idx += 1;
                assignee = args.get(idx).copied();
            }
            "--tag" => {
                idx += 1;
                if let Some(tag) = args.get(idx) {
                    tags.push(tag.to_ascii_lowercase());
                }
            }
            other => {
                writeln!(out, "unbekannte Option: {other}")?;
                return Ok(());
            }
        }
        idx += 1;
    }

    if title.is_none() || description.is_none() || reporter.is_none() {
        writeln!(out, "Nutzung: create ticket --title <titel> --description <text> --reporter <username> [--priority <prio>] [--assignee <username>] [--tag <tag> ...]")?;
        return Ok(());
    }

    let reporter_user = match users.find_by_username(reporter.unwrap()) {
        Ok(Some(user)) => user,
        Ok(None) => {
            writeln!(out, "Reporter {} nicht gefunden", reporter.unwrap())?;
            return Ok(());
        }
        Err(err) => {
            writeln!(out, "Fehler: {err}")?;
            return Ok(());
        }
    };

    let assignee_id = if let Some(name) = assignee {
        match users.find_by_username(name) {
            Ok(Some(user)) => Some(user.id),
            Ok(None) => {
                writeln!(out, "Bearbeiter {name} nicht gefunden")?;
                return Ok(());
            }
            Err(err) => {
                writeln!(out, "Fehler: {err}")?;
                return Ok(());
            }
        }
    } else {
        None
    };

    let command = CreateTicketCommand::new(
        title.unwrap(),
        description.unwrap(),
        priority,
        reporter_user.id,
    )
    .with_assignee(assignee_id)
    .with_tags(tags);

    match service.create(command) {
        Ok(ticket) => {
            writeln!(out, "Ticket {} angelegt", ticket.id)?;
        }
        Err(err) => writeln!(out, "Fehler beim Anlegen: {err}")?,
    }
    Ok(())
}

pub(crate) fn update_assignment(
    service: &TicketService,
    users: &UserService,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    if args.len() < 2 {
        writeln!(out, "Nutzung: assign ticket <ticket-id> <username|none>")?;
        return Ok(());
    }
    let ticket_id = match TicketId::from_str(args[0]) {
        Ok(id) => id,
        Err(err) => {
            writeln!(out, "Ungültige Ticket-ID: {err}")?;
            return Ok(());
        }
    };
    let assignment = args[1];
    let assignee_id = if assignment.eq_ignore_ascii_case("none") {
        None
    } else {
        match users.find_by_username(assignment) {
            Ok(Some(user)) => Some(user.id),
            Ok(None) => {
                writeln!(out, "Benutzer {assignment} nicht gefunden")?;
                return Ok(());
            }
            Err(err) => {
                writeln!(out, "Fehler: {err}")?;
                return Ok(());
            }
        }
    };

    match service.assign(&ticket_id, assignee_id) {
        Ok(ticket) => {
            writeln!(out, "Ticket {} aktualisiert", ticket.id)?;
        }
        Err(err) => writeln!(out, "Fehler: {err}")?,
    }
    Ok(())
}

pub(crate) fn update_status(
    service: &TicketService,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    if args.len() < 2 {
        writeln!(out, "Nutzung: status ticket <ticket-id> <status>")?;
        return Ok(());
    }
    let ticket_id = match TicketId::from_str(args[0]) {
        Ok(id) => id,
        Err(err) => {
            writeln!(out, "Ungültige Ticket-ID: {err}")?;
            return Ok(());
        }
    };
    let status = match TicketStatus::from_str(args[1]) {
        Ok(status) => status,
        Err(err) => {
            writeln!(out, "Ungültiger Status: {err}")?;
            return Ok(());
        }
    };

    match service.transition_status(&ticket_id, status) {
        Ok(ticket) => {
            writeln!(
                out,
                "Ticket {} Status aktualisiert auf {}",
                ticket.id, ticket.status
            )?;
        }
        Err(err) => writeln!(out, "Fehler: {err}")?,
    }
    Ok(())
}

pub(crate) fn show_ticket(
    service: &TicketService,
    users: &UserService,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<()> {
    let ticket_id = match args.first() {
        Some(id) => match TicketId::from_str(id) {
            Ok(id) => id,
            Err(err) => {
                writeln!(out, "Ungültige Ticket-ID: {err}")?;
                return Ok(());
            }
        },
        None => {
            writeln!(out, "Nutzung: show ticket <ticket-id>")?;
            return Ok(());
        }
    };

    match service.find_by_id(&ticket_id) {
        Ok(Some(ticket)) => {
            let reporter = users
                .find_by_id(&ticket.reporter_id)
                .ok()
                .flatten()
                .map(|user| user.username)
                .unwrap_or_else(|| "?".to_string());
            let assignee = ticket
                .assignee_id
                .and_then(|id| users.find_by_id(&id).ok().flatten())
                .map(|user| user.username)
                .unwrap_or_else(|| "-".to_string());
            writeln!(out, "Ticket: {}", ticket.id)?;
            writeln!(out, "Titel: {}", ticket.title)?;
            writeln!(out, "Status: {}", ticket.status)?;
            writeln!(out, "Priorität: {}", ticket.priority)?;
            writeln!(out, "Reporter: {}", reporter)?;
            writeln!(out, "Assignee: {}", assignee)?;
            writeln!(
                out,
                "Erstellt: {}",
                utils::format_relative_time(ticket.created_at)
            )?;
            writeln!(
                out,
                "Aktualisiert: {}",
                utils::format_relative_time(ticket.updated_at)
            )?;
            if ticket.tags.is_empty() {
                writeln!(out, "Tags: -")?;
            } else {
                writeln!(out, "Tags: {}", ticket.tags.join(", "))?;
            }
            writeln!(out, "Beschreibung:\n{}", ticket.description)?;
        }
        Ok(None) => writeln!(out, "Ticket {} nicht gefunden", ticket_id)?,
        Err(err) => writeln!(out, "Fehler: {err}")?,
    }
    Ok(())
}

fn build_user_index(service: &UserService) -> std::collections::HashMap<UserId, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(users) = service.list(UserFilter::default()) {
        for user in users {
            map.insert(user.id, user.username);
        }
    }
    map
}

fn truncate(input: &str, len: usize) -> String {
    if input.chars().count() <= len {
        input.to_string()
    } else {
        let mut result: String = input.chars().take(len).collect();
        result.push('…');
        result
    }
}
