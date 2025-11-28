use std::io::{self, Write};

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionKind, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::utils::messages::cli::builtins::help::{
    command as help_messages, handler as help_handler_messages,
};

const HELP_ARGUMENTS: &[CommandArgument] = &[CommandArgument {
    name: "command",
    optional: true,
    variadic: false,
    completion: CompletionKind::None,
}];

const HELP_SHAPE: CommandShape = CommandShape::new("help", &[], HELP_ARGUMENTS, &[]);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        help_messages::NAME,
        help_messages::DESCRIPTION,
        help_messages::USAGE,
        help_messages::DETAILS,
        handle,
        HELP_SHAPE,
    )
}

fn handle(
    _deps: &CliDependencies,
    args: &[&str],
    registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    if let Some(command_name) = args.first() {
        match registry.get(command_name) {
            Some(entry) => {
                writeln!(
                    out,
                    "{}",
                    help_handler_messages::describe_command(&entry.name, &entry.description)
                )?;
                writeln!(out, "{}", help_handler_messages::usage_line(entry.usage))?;
                if !entry.aliases.is_empty() {
                    writeln!(
                        out,
                        "{}",
                        help_handler_messages::aliases_line(&entry.aliases)
                    )?;
                }
                if !entry.details.is_empty() {
                    writeln!(out, "{}", help_handler_messages::DETAILS_HEADER)?;
                    for line in entry.details {
                        writeln!(out, "{}", help_handler_messages::detail_line(line))?;
                    }
                }
            }
            None => {
                writeln!(
                    out,
                    "{}",
                    help_handler_messages::unknown_command(command_name)
                )?;
            }
        }
    } else {
        writeln!(out, "{}", help_handler_messages::COMMANDS_HEADER)?;
        let mut table = Table::new(
            help_handler_messages::TABLE_HEADERS
                .iter()
                .map(|entry| entry.to_string())
                .collect(),
        );
        for entry in registry.entries() {
            let command_name = if entry.aliases.is_empty() {
                entry.name.clone()
            } else {
                format!("{} [{}]", entry.name, entry.aliases.join(", "))
            };
            table.add_row(vec![
                command_name,
                entry.usage.to_string(),
                entry.description.clone(),
            ]);
        }
        table.render(out, "  ")?;
        writeln!(out, "{}", help_handler_messages::DETAILS_HINT)?;
    }
    Ok(CommandOutcome::Continue)
}
