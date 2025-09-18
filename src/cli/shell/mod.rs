use crate::cli::commands::builtins::{db_shell, help};
use rustyline::{error::ReadlineError, DefaultEditor};
use std::io::{self, Write};

const BANNER: &str = r#"
______               _                 
|  ___| __ ___  _ __| | ___  _ __ ___  
| |_ | '__/ _ \| '__| |/ _ \| '_ ` _ \ 
|  _|| | | (_) | |  | | (_) | | | | | |
|_|  |_|  \___/|_|  |_|\___/|_| |_| |_|
"#;

pub fn run_shell() -> io::Result<()> {
    let mut stdout = io::stdout();
    writeln!(&mut stdout, "{BANNER}")?;
    writeln!(
        &mut stdout,
        "Willkommen beim Fenrir CLI! Tippe 'help' für verfügbare Befehle.\n"
    )?;

    let mut editor = DefaultEditor::new().map_err(map_readline_error)?;

    loop {
        match editor.readline("> ") {
            Ok(line) => {
                let cmd = line.trim();
                if cmd.is_empty() {
                    continue;
                }

                editor.add_history_entry(cmd);
                match cmd {
                    "help" => {
                        help::run(&mut stdout)?;
                    }
                    "db-shell" => {
                        db_shell::run_db_shell()?;
                    }
                    "exit" => break,
                    _ => writeln!(&mut stdout, "unknown command: {}", cmd)?,
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
    Ok(())
}

fn map_readline_error(err: ReadlineError) -> io::Error {
    match err {
        ReadlineError::Io(err) => err,
        other => io::Error::new(io::ErrorKind::Other, other.to_string()),
    }
}
