use std::io::{self, Write};

pub fn run(mut out: impl Write) -> io::Result<()> {
    writeln!(out, "available commands: help, db-shell, exit")
}
