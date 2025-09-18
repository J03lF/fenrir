use crate::cli::commands::builtins::{db_shell, help};
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
    let mut stdin = io::stdin();

    writeln!(&mut stdout, "{BANNER}")?;
    writeln!(
        &mut stdout,
        "Willkommen beim Fenrir CLI! Tippe 'help' für verfügbare Befehle.\n"
    )?;

    let mut buffer = String::new();

    loop {
        write!(&mut stdout, "> ")?;
        stdout.flush()?;
        buffer.clear();

        match stdin.read_line(&mut buffer) {
            Ok(0) => {
                writeln!(&mut stdout)?;
                break;
            }
            Ok(_) => {
                let cmd = buffer.trim();
                if cmd.is_empty() {
                    continue;
                }

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
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {
                writeln!(&mut stdout, "\nAbgebrochen (Strg+C erkannt).")?;
                break;
            }
            Err(err) => {
                writeln!(&mut stdout, "Eingabefehler: {err}")?;
                break;
            }
        }
    }

    Ok(())
}
