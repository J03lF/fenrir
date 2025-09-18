use std::io::{self, Write};

pub fn run_db_shell() -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        write!(stdout, "db: ")?;
        stdout.flush()?;
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let cmd = line.trim();
        match cmd {
            "exit" => break,
            "help" => writeln!(stdout, "db-shell: stub. commands: help, exit")?,
            _ if cmd.is_empty() => {}
            _ => writeln!(stdout, "stub: received '{}', no DB connected yet", cmd)?,
        }
    }
    Ok(())
}
