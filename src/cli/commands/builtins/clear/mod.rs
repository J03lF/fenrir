use crate::cli::commands::registry::{
    CliDependencies, CommandEntry, CommandOutcome, CommandRegistry, CommandShape, ShellEnvironment,
};
use crate::prompts;
use std::io::{self, Write};

const DETAILS: &[&str] = &[
    "clear – löscht den sichtbaren Bildschirminhalt",
    "Alias für das klassische Terminal-Kommando, nützlich in SSH/CLI-Sitzungen",
];

const CLEAR_SHAPE: CommandShape = CommandShape::basic("clear");

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "clear",
        "Leert den Bildschirm und setzt den Cursor zurück",
        "clear",
        DETAILS,
        handle,
        CLEAR_SHAPE,
    )
}

fn handle(
    _deps: &CliDependencies,
    _args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    write!(out, "{}", prompts::clear_screen_sequence())?;
    out.flush()?;
    Ok(CommandOutcome::Continue)
}
