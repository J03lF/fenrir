use rustyline::{error::ReadlineError, DefaultEditor};
use std::io::{self, Write};

pub fn run_db_shell() -> io::Result<()> {
    let mut stdout = io::stdout();
    writeln!(
        &mut stdout,
        "DB-Shell (Stub) gestartet. Tippe 'help' für Befehle oder 'exit' zum Beenden."
    )?;

    let mut editor = DefaultEditor::new().map_err(map_readline_error)?;

    loop {
        match editor.readline("db: ") {
            Ok(line) => {
                let cmd = line.trim();
                if cmd.is_empty() {
                    continue;
                }

                editor.add_history_entry(cmd);
                match cmd {
                    "exit" => break,
                    "help" => writeln!(&mut stdout, "db-shell: stub. commands: help, exit")?,
                    _ => writeln!(&mut stdout, "stub: received '{}', no DB connected yet", cmd)?,
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
