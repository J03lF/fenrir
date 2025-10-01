use std::io::{self, Write};
use std::str::FromStr;

use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::domain::user::{UserFilter, UserRole};
use crate::services::user::RegisterUserCommand;
use crate::services::UserService;
use crate::utils;

pub fn command() -> CommandEntry {
    CommandEntry::new(
        "user",
        "Verwaltet Benutzer (listen, anlegen, sperren)",
        handle,
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
    match action {
        "list" => list_users(&deps.services.user, args.get(1..).unwrap_or_default(), out)?,
        "show" => show_user(&deps.services.user, args.get(1..).unwrap_or_default(), out)?,
        "create" => create_user(&deps.services.user, args.get(1..).unwrap_or_default(), out)?,
        "lock" => set_lock_state(
            &deps.services.user,
            args.get(1..).unwrap_or_default(),
            true,
            out,
        )?,
        "unlock" => set_lock_state(
            &deps.services.user,
            args.get(1..).unwrap_or_default(),
            false,
            out,
        )?,
        other => {
            writeln!(out, "unbekannte Aktion: {other}")?;
            writeln!(out, "verfügbar: user [list|show|create|lock|unlock]")?;
        }
    }
    Ok(CommandOutcome::Continue)
}

fn list_users(service: &UserService, args: &[&str], out: &mut dyn Write) -> io::Result<()> {
    let mut filter = UserFilter::default();
    let mut idx = 0;
    while idx < args.len() {
        match args[idx] {
            "--role" => {
                idx += 1;
                if let Some(role) = args.get(idx) {
                    match UserRole::from_str(role) {
                        Ok(role) => filter.role = Some(role),
                        Err(err) => {
                            writeln!(out, "Rollenfehler: {err}")?;
                            return Ok(());
                        }
                    }
                }
            }
            "--include-locked" => filter.include_locked = true,
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

    match service.list(filter) {
        Ok(users) if users.is_empty() => writeln!(out, "Keine Benutzer gefunden")?,
        Ok(users) => {
            let mut table = Table::new(vec![
                "Lock".to_string(),
                "Username".to_string(),
                "E-Mail".to_string(),
                "Rollen".to_string(),
                "Name".to_string(),
            ]);
            for user in users {
                let roles = user
                    .roles
                    .iter()
                    .map(UserRole::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                let lock_state = if user.is_locked { "🔒" } else { "" }.to_string();
                table.add_row(vec![
                    lock_state,
                    user.username.clone(),
                    user.email.to_string(),
                    roles,
                    user.display_name.unwrap_or_default(),
                ]);
            }
            table.render(out, "  ")?;
        }
        Err(err) => writeln!(out, "Fehler bei Benutzerliste: {err}")?,
    }
    Ok(())
}

fn show_user(service: &UserService, args: &[&str], out: &mut dyn Write) -> io::Result<()> {
    let username = match args.first() {
        Some(username) => *username,
        None => {
            writeln!(out, "Nutzung: user show <username>")?;
            return Ok(());
        }
    };
    match service.find_by_username(username) {
        Ok(Some(user)) => {
            writeln!(out, "ID: {}", user.id)?;
            writeln!(out, "Username: {}", user.username)?;
            writeln!(out, "E-Mail: {}", user.email)?;
            writeln!(
                out,
                "Rollen: {}",
                user.roles
                    .iter()
                    .map(UserRole::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
            writeln!(
                out,
                "Gesperrt: {}",
                if user.is_locked { "ja" } else { "nein" }
            )?;
            writeln!(
                out,
                "Angelegt: {}",
                utils::format_relative_time(user.created_at)
            )?;
            writeln!(
                out,
                "Aktualisiert: {}",
                utils::format_relative_time(user.updated_at)
            )?;
        }
        Ok(None) => writeln!(out, "Benutzer {username} nicht gefunden")?,
        Err(err) => writeln!(out, "Fehler: {err}")?,
    }
    Ok(())
}

fn create_user(service: &UserService, args: &[&str], out: &mut dyn Write) -> io::Result<()> {
    let mut username: Option<&str> = None;
    let mut email: Option<&str> = None;
    let mut display_name: Option<String> = None;
    let mut roles: Vec<UserRole> = Vec::new();

    let mut idx = 0;
    while idx < args.len() {
        match args[idx] {
            "--username" => {
                idx += 1;
                username = args.get(idx).copied();
            }
            "--email" => {
                idx += 1;
                email = args.get(idx).copied();
            }
            "--display-name" => {
                idx += 1;
                if let Some(value) = args.get(idx) {
                    display_name = Some((*value).to_string());
                }
            }
            "--role" => {
                idx += 1;
                if let Some(role) = args.get(idx) {
                    match UserRole::from_str(role) {
                        Ok(role) => roles.push(role),
                        Err(err) => {
                            writeln!(out, "Ungültige Rolle: {err}")?;
                            return Ok(());
                        }
                    }
                }
            }
            other => {
                writeln!(out, "unbekannte Option: {other}")?;
                return Ok(());
            }
        }
        idx += 1;
    }

    if username.is_none() || email.is_none() {
        writeln!(out, "Nutzung: user create --username <name> --email <adresse> [--display-name <name>] [--role <rolle> ...]")?;
        return Ok(());
    }
    if roles.is_empty() {
        roles.push(UserRole::Viewer);
    }

    match service.register(RegisterUserCommand::new(
        username.unwrap(),
        email.unwrap(),
        display_name,
        roles,
    )) {
        Ok(user) => {
            writeln!(out, "Benutzer {} angelegt (ID: {})", user.username, user.id)?;
        }
        Err(err) => {
            writeln!(out, "Fehler beim Anlegen: {err}")?;
        }
    }
    Ok(())
}

fn set_lock_state(
    service: &UserService,
    args: &[&str],
    locked: bool,
    out: &mut dyn Write,
) -> io::Result<()> {
    let username = match args.first() {
        Some(username) => *username,
        None => {
            writeln!(
                out,
                "Nutzung: user {} <username>",
                if locked { "lock" } else { "unlock" }
            )?;
            return Ok(());
        }
    };
    let maybe_user = service.find_by_username(username);
    match maybe_user {
        Ok(Some(user)) => match service.set_lock_state(&user.id, locked) {
            Ok(updated) => {
                writeln!(
                    out,
                    "Benutzer {} ist jetzt {}",
                    updated.username,
                    if locked { "gesperrt" } else { "entsperrt" }
                )?;
            }
            Err(err) => writeln!(out, "Fehler: {err}")?,
        },
        Ok(None) => writeln!(out, "Benutzer {username} nicht gefunden")?,
        Err(err) => writeln!(out, "Fehler: {err}")?,
    }
    Ok(())
}
