use super::{ticket, user};
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionContext, CompletionKind, ShellEnvironment,
};
use std::io::{self, Write};

const CREATE_RESOURCE_OPTIONS: &[&str] = &["user", "users", "ticket", "tickets"];

const CREATE_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(CREATE_RESOURCE_OPTIONS));
const CREATE_OPTION_ARGUMENT: CommandArgument = CommandArgument::optional("option")
    .with_completion(CompletionKind::Dynamic(complete_create_options))
    .variadic();

const CREATE_ARGUMENTS: &[CommandArgument] = &[CREATE_RESOURCE_ARGUMENT, CREATE_OPTION_ARGUMENT];
const CREATE_SHAPE: CommandShape = CommandShape::new("create", &[], CREATE_ARGUMENTS, &[]);

const CREATE_DETAILS: &[&str] = &[
    "create user --username <name> --email <adresse> [--role <rolle>] ...",
    "create ticket --title <titel> --description <text> --priority <prio> ...",
];

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "create",
        "Erstellt Benutzer oder Tickets",
        "create <user|ticket> [optionen]",
        CREATE_DETAILS,
        handle_create,
        CREATE_SHAPE,
    )
}

fn handle_create(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "Nutzung: create <user|ticket> [optionen]")?;
        return Ok(CommandOutcome::Continue);
    };

    match resource.to_ascii_lowercase().as_str() {
        "user" | "users" => {
            user::create_user(&deps.services.user, tail, out)?;
        }
        "ticket" | "tickets" => {
            ticket::create_ticket(&deps.services.ticket, &deps.services.user, tail, out)?;
        }
        other => {
            writeln!(out, "unbekannte Ressource: {other}")?;
            writeln!(out, "verfügbar: create user|ticket")?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn complete_create_options(_deps: &CliDependencies, ctx: &CompletionContext<'_>) -> Vec<String> {
    let Some(resource) = ctx.tokens.get(1).copied() else {
        return Vec::new();
    };

    match resource {
        "user" | "users" => user::USER_CREATE_OPTIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        "ticket" | "tickets" => ticket::TICKET_CREATE_OPTIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        _ => Vec::new(),
    }
}
