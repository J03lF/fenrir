use crate::cli::commands::builtins::{db_shell, help};
use std::io::{self, Write};

pub fn run_shell() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        write!(stdout, "> ")?;
        stdout.flush()?;
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let cmd = line.trim();
        match cmd {
            "help" => {
                help::run(&mut stdout)?;
            }
            "db-shell" => {
                db_shell::run_db_shell()?;
            }
            "exit" => break,
            "" => {}
            _ => writeln!(stdout, "unknown command: {}", cmd)?,
        }
    }
    Ok(())
}
