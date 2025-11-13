use std::io::{self, Write};
use std::sync::Arc;

use time::format_description::well_known::Rfc3339;

use crate::audit::AuditActor;
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionKind, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::security::auth::Role;
use crate::security::identity::{IdentityError, IdentityProvider, IssueTokenRequest};
use tokio::runtime::Handle;
use tokio::task::block_in_place;

const USER_DETAILS: &[&str] = &[
    "user list – zeigt registrierte Identity-Benutzer",
    "user issue <user> [--role <admin|operator|viewer>] [--display-name <name>] – stellt einen Control-Plane-Token aus",
    "user tokens <user> – zeigt vergebene Token (Fingerprints, Laufzeiten) für einen Benutzer",
];

const ROLE_OPTIONS: &[&str] = &["admin", "operator", "viewer"];

const USER_ARGUMENTS: &[CommandArgument] = &[CommandArgument::required("action")
    .with_completion(CompletionKind::Static(&["list", "issue", "tokens"]))
    .variadic()];

const USER_SHAPE: CommandShape = CommandShape::new("user", &[], USER_ARGUMENTS, &[]);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "user",
        "Verwaltet Control-Plane-Identitäten",
        "user <list|issue> …",
        USER_DETAILS,
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
        writeln!(out, "Nutzung: user <list|issue> …")?;
        return Ok(CommandOutcome::Continue);
    };

    match action.to_ascii_lowercase().as_str() {
        "list" => handle_list(deps, out),
        "issue" => handle_issue(deps, rest, out),
        "tokens" => handle_tokens(deps, rest, out),
        other => {
            writeln!(
                out,
                "Unbekannte Aktion '{other}'. Verfügbare Aktionen: list, issue, tokens"
            )?;
            Ok(CommandOutcome::Continue)
        }
    }
}

fn handle_list(deps: &CliDependencies, out: &mut dyn Write) -> io::Result<CommandOutcome> {
    let Some(identity) = deps.services.identity() else {
        writeln!(out, "Identity-Service ist nicht verfügbar")?;
        return Ok(CommandOutcome::Continue);
    };

    let users = call_identity(Arc::clone(&identity), |identity| identity.list_users())?;

    if users.is_empty() {
        writeln!(out, "Keine Identity-Benutzer registriert.")?;
        return Ok(CommandOutcome::Continue);
    }

    let mut table = Table::new(vec![
        "User".to_string(),
        "Rolle".to_string(),
        "Tokens".to_string(),
        "Zuletzt Ausgestellt".to_string(),
    ]);

    for user in users {
        let label = user
            .display_name
            .clone()
            .unwrap_or_else(|| user.user_id.clone());
        let last_issued = user
            .last_issued_at
            .as_ref()
            .map(|ts| {
                ts.format(&Rfc3339)
                    .unwrap_or_else(|_| "<invalid>".to_string())
            })
            .unwrap_or_else(|| "-".to_string());

        table.add_row(vec![
            label,
            user.role.as_str().to_string(),
            user.token_count.to_string(),
            last_issued,
        ]);
    }

    table.render(out, "")?;
    Ok(CommandOutcome::Continue)
}

fn handle_issue(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some((user_id, rest)) = args.split_first() else {
        writeln!(
            out,
            "Nutzung: user issue <user> [--role <admin|operator|viewer>] [--display-name <name>]"
        )?;
        return Ok(CommandOutcome::Continue);
    };

    let Some(identity) = deps.services.identity() else {
        writeln!(out, "Identity-Service ist nicht verfügbar")?;
        return Ok(CommandOutcome::Continue);
    };

    let mut role = Role::Admin;
    let mut display_name: Option<String> = None;
    let mut idx = 0;
    while idx < rest.len() {
        match rest[idx] {
            "--role" | "-r" => {
                idx += 1;
                let Some(value) = rest.get(idx) else {
                    writeln!(out, "--role erwartet einen Wert ({ROLE_OPTIONS:?})")?;
                    return Ok(CommandOutcome::Continue);
                };
                role = parse_role(value, out)?;
            }
            "--display-name" | "--name" => {
                idx += 1;
                let Some(value) = rest.get(idx) else {
                    writeln!(out, "--display-name erwartet einen Wert")?;
                    return Ok(CommandOutcome::Continue);
                };
                display_name = Some(value.to_string());
            }
            other => {
                writeln!(out, "Unbekannte Option '{other}'")?;
                return Ok(CommandOutcome::Continue);
            }
        }
        idx += 1;
    }

    let user_id_string = user_id.to_string();
    let role_clone = role.clone();
    let display_name_clone = display_name.clone();
    let issued = call_identity(identity, move |identity| {
        identity.issue_token(IssueTokenRequest {
            actor: AuditActor::System,
            user_id: user_id_string,
            display_name: display_name_clone,
            role: role_clone,
        })
    })?;

    writeln!(out, "Token für '{}' ({})", user_id, role.as_str())?;
    writeln!(out, "Token-ID: {}", issued.token_id)?;
    writeln!(out, "Fingerprint: {}", issued.fingerprint)?;
    writeln!(
        out,
        "Gültig bis: {}",
        issued
            .expires_at
            .format(&Rfc3339)
            .unwrap_or_else(|_| "<invalid>".to_string())
    )?;
    writeln!(out, "Token (kopieren & sicher speichern):")?;
    writeln!(out, "{}", issued.token)?;

    Ok(CommandOutcome::Continue)
}

fn handle_tokens(
    deps: &CliDependencies,
    args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    let Some((user_id, _)) = args.split_first() else {
        writeln!(out, "Nutzung: user tokens <user>")?;
        return Ok(CommandOutcome::Continue);
    };

    let Some(identity) = deps.services.identity() else {
        writeln!(out, "Identity-Service ist nicht verfügbar")?;
        return Ok(CommandOutcome::Continue);
    };

    let users = call_identity(Arc::clone(&identity), |identity| identity.list_users())?;
    if let Some(user) = users.into_iter().find(|u| u.user_id == *user_id) {
        if user.tokens.is_empty() {
            writeln!(out, "Keine Token für '{user_id}' vorhanden.")?;
            return Ok(CommandOutcome::Continue);
        }

        let mut table = Table::new(vec![
            "Token-ID".to_string(),
            "Fingerprint".to_string(),
            "Ausgestellt".to_string(),
            "Gültig bis".to_string(),
            "Key-ID".to_string(),
        ]);

        for token in user.tokens {
            let issued = token
                .issued_at
                .format(&Rfc3339)
                .unwrap_or_else(|_| "<invalid>".to_string());
            let expires = token
                .expires_at
                .format(&Rfc3339)
                .unwrap_or_else(|_| "<invalid>".to_string());
            table.add_row(vec![
                token.token_id,
                token.fingerprint,
                issued,
                expires,
                token.key_id,
            ]);
        }

        table.render(out, "")?;
    } else {
        writeln!(out, "Identity-Benutzer '{user_id}' nicht gefunden.")?;
    }

    Ok(CommandOutcome::Continue)
}

fn call_identity<T, F>(identity: Arc<dyn IdentityProvider>, func: F) -> io::Result<T>
where
    T: Send + 'static,
    F: FnOnce(Arc<dyn IdentityProvider>) -> Result<T, IdentityError> + Send + 'static,
{
    if Handle::try_current().is_ok() {
        block_in_place(|| func(identity))
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))
    } else {
        func(identity).map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))
    }
}

fn parse_role(value: &str, out: &mut dyn Write) -> io::Result<Role> {
    match value.to_ascii_lowercase().as_str() {
        "admin" => Ok(Role::Admin),
        "operator" => Ok(Role::Operator),
        "viewer" => Ok(Role::Viewer),
        other => {
            writeln!(
                out,
                "Unbekannte Rolle '{other}'. Erlaubt: admin, operator, viewer"
            )?;
            Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid role"))
        }
    }
}
