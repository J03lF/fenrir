use crate::cli::commands::builtins;
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandStatus, ShellEnvironment,
};
use crate::cli::completion::SimpleCompleter;
use crate::config::AppConfig;
use crate::prompts;
use crate::services::{AppServices, ServiceStatus};
use rustyline::history::{DefaultHistory, History};
use rustyline::{error::ReadlineError, Editor};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

pub fn run_shell(config: Arc<AppConfig>, services: Arc<AppServices>) -> io::Result<()> {
    let mut stdout = io::stdout();
    // Clear screen and position cursor in the top left before showing the banner.
    write!(&mut stdout, "{}", prompts::clear_screen_sequence())?;
    writeln!(&mut stdout, "{}", prompts::banner())?;
    writeln!(&mut stdout, "{}", prompts::welcome_line(config.as_ref()))?;

    let registry = builtins::build_registry();
    let dependencies = CliDependencies::new(Arc::clone(&config), Arc::clone(&services));

    services.registry().set_status(
        "cli-shell",
        ServiceStatus::Active,
        Some(format!("lokale Sitzung pid={}", std::process::id())),
    );

    let mut editor =
        Editor::<SimpleCompleter, DefaultHistory>::new().map_err(map_readline_error)?;
    editor.set_helper(Some(SimpleCompleter::new(registry.command_names())));
    let history_path = init_history(&mut editor);
    let prompt_set = prompts::prompt_set(config.as_ref());

    loop {
        match editor.readline(&prompt_set.main_cli) {
            Ok(line) => {
                let cmd = line.trim();
                if cmd.is_empty() {
                    continue;
                }

                let _ = editor.add_history_entry(cmd);

                let mut parts = cmd.split_whitespace();
                if let Some(name) = parts.next() {
                    let args: Vec<&str> = parts.collect();
                    match registry.execute(
                        name,
                        &args,
                        &dependencies,
                        &mut stdout,
                        ShellEnvironment::Cli,
                    )? {
                        CommandStatus::Executed(CommandOutcome::Continue) => {}
                        CommandStatus::Executed(CommandOutcome::ExitShell) => break,
                        CommandStatus::Executed(CommandOutcome::EnterDbShell) => {
                            let session = services.db_shell.create_session();
                            if let Err(err) =
                                crate::cli::commands::builtins::db_shell::run_local_db_shell(
                                    session,
                                    prompt_set.db_cli.clone(),
                                )
                            {
                                writeln!(&mut stdout, "db-shell Fehler: {err}")?;
                                services.registry().set_status(
                                    "db-shell",
                                    ServiceStatus::Degraded,
                                    Some(format!("Fehler: {err}")),
                                );
                            } else {
                                services.registry().set_status(
                                    "db-shell",
                                    ServiceStatus::Active,
                                    Some("Bereit für neue Sessions".to_string()),
                                );
                            }
                        }
                        CommandStatus::NotFound => {
                            writeln!(&mut stdout, "unbekannter Befehl: {}", name)?;
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(ReadlineError::Io(err)) => return Err(err),
            Err(err) => {
                writeln!(&mut stdout, "Eingabefehler: {err}")?;
                break;
            }
        }
    }
    if let Some(path) = history_path {
        let _ = editor.save_history(&path);
    }
    services.registry().set_status(
        "cli-shell",
        ServiceStatus::Standby,
        Some("Wartet auf nächste Sitzung".to_string()),
    );
    Ok(())
}

fn init_history<H, I>(editor: &mut Editor<H, I>) -> Option<PathBuf>
where
    H: rustyline::Helper,
    I: History,
{
    history_path().map(|path| {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = editor.load_history(&path);
        path
    })
}

fn history_path() -> Option<PathBuf> {
    let mut path = std::env::var_os("FENRIR_HISTORY_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))?;
    path.push("fenrir");
    path.push("cli");
    path.push("history.txt");
    Some(path)
}

fn map_readline_error(err: ReadlineError) -> io::Error {
    match err {
        ReadlineError::Io(err) => err,
        other => io::Error::new(io::ErrorKind::Other, other.to_string()),
    }
}
