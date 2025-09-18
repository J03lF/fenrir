use std::io::{self, Write};

pub fn run_db_shell() -> io::Result<()> {
    let mut stdout = io::stdout();
    let mut stdin = io::stdin();
    let mut buffer = String::new();

    writeln!(
        &mut stdout,
        "DB-Shell (Stub) gestartet. Tippe 'help' für Befehle oder 'exit' zum Beenden."
    )?;

    loop {
        write!(&mut stdout, "db: ")?;
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
                    "exit" => break,
                    "help" => writeln!(&mut stdout, "db-shell: stub. commands: help, exit")?,
                    _ => writeln!(&mut stdout, "stub: received '{}', no DB connected yet", cmd)?,
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
