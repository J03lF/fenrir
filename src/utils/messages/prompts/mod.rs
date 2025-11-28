#[allow(clippy::too_many_arguments)]
pub fn main_prompt(
    role_color: &str,
    role: &str,
    accent: &str,
    transport: &str,
    primary: &str,
    user: &str,
    host: &str,
    dim: &str,
    app: &str,
    reset: &str,
) -> String {
    format!(
        "[{role_color}{role}{reset}::{accent}{transport}{reset}] {primary}{user}@{host}{reset} {dim}{app}{reset} {dim}»{reset} ",
        role_color = role_color,
        role = role,
        reset = reset,
        accent = accent,
        transport = transport,
        primary = primary,
        user = user,
        host = host,
        dim = dim,
        app = app,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn db_prompt(
    role_color: &str,
    role: &str,
    accent: &str,
    transport: &str,
    primary: &str,
    user: &str,
    host: &str,
    db_color: &str,
    dim: &str,
    app: &str,
    reset: &str,
) -> String {
    format!(
        "[{role_color}{role}{reset}::{accent}{transport}{reset}] {primary}{user}@{host}{reset} {db_color}db{reset} {dim}{app}{reset} {dim}»{reset} ",
        role_color = role_color,
        role = role,
        reset = reset,
        accent = accent,
        transport = transport,
        primary = primary,
        user = user,
        host = host,
        db_color = db_color,
        dim = dim,
        app = app,
    )
}

pub fn welcome_line(
    dim: &str,
    accent: &str,
    server_name: &str,
    primary: &str,
    app_name: &str,
    version: &str,
    reset: &str,
) -> String {
    format!(
        "{dim}Willkommen auf {accent}{server}{dim} - {primary}{app} v{version}{reset}",
        dim = dim,
        accent = accent,
        server = server_name,
        primary = primary,
        app = app_name,
        version = version,
        reset = reset,
    )
}

pub fn help_hint(dim: &str, reset: &str) -> String {
    format!(
        "{dim}Tippe 'help' zur Anzeige aller verfügbaren Befehle.{reset}",
        dim = dim,
        reset = reset,
    )
}
