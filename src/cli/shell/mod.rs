use crate::cli::commands::builtins;
use crate::cli::commands::registry::{CommandOutcome, CommandStatus, ShellEnvironment};
use crate::config::AppConfig;
use crate::prompts;
use rustyline::{error::ReadlineError, DefaultEditor};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

pub fn run_shell(config: &AppConfig) -> io::Result<()> {
    let mut stdout = io::stdout();
    // Clear screen and position cursor in the top left before showing the banner.
    write!(&mut stdout, "{}", prompts::clear_screen_sequence())?;
    writeln!(&mut stdout, "{}", prompts::banner())?;
    writeln!(&mut stdout, "{}", prompts::welcome_line(config))?;

    let registry = builtins::build_registry();

    let mut editor = DefaultEditor::new().map_err(map_readline_error)?;
    let history_path = init_history(&mut editor);
    let prompt_set = prompts::prompt_set(config);

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
                        config,
                        &mut stdout,
                        ShellEnvironment::Cli,
                    )? {
                        CommandStatus::Executed(CommandOutcome::Continue) => {}
                        CommandStatus::Executed(CommandOutcome::ExitShell) => break,
                        CommandStatus::Executed(CommandOutcome::EnterDbShell) => {
                            if let Err(err) =
                                crate::cli::commands::builtins::db_shell::run_local_db_shell()
                            {
                                writeln!(&mut stdout, "db-shell Fehler: {err}")?;
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
    Ok(())
}

fn init_history(editor: &mut DefaultEditor) -> Option<PathBuf> {
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
