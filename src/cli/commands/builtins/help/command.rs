use std::io::{self, Write};

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionContext, CompletionKind, ShellEnvironment,
};
use crate::cli::commands::table::Table;
use crate::cli::output::style::FieldStyle;
use crate::cli::output::{MessageBox, StatusBox};
use crate::utils::messages::cli::builtins::help::{
    command as help_messages, handler as help_handler_messages,
};

fn complete_commands(_deps: &CliDependencies, ctx: &CompletionContext<'_>) -> Vec<String> {
    let registry = crate::cli::commands::builtins::build_registry();
    registry
        .entries()
        .flat_map(|entry| {
            let mut names = vec![entry.name.clone()];
            names.extend(entry.aliases.iter().map(|a| a.to_string()));
            names
        })
        .filter(|name| ctx.prefix.is_empty() || name.starts_with(ctx.prefix))
        .collect()
}

const HELP_ARGUMENTS: &[CommandArgument] = &[CommandArgument {
    name: "command",
    optional: true,
    variadic: false,
    completion: CompletionKind::Dynamic(complete_commands),
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
                render_command_details(out, entry)?;
            }
            None => {
                // Command not found - show suggestions
                let suggestions = registry.find_similar(command_name);
                let mut msg_box =
                    MessageBox::warning(format!("Unknown command: '{}'", command_name));

                if !suggestions.is_empty() {
                    msg_box = msg_box.message("Did you mean?");
                    for suggestion in suggestions {
                        msg_box = msg_box.suggestion(suggestion);
                    }
                } else {
                    msg_box = msg_box.suggestion("Type 'help' to see all commands");
                }

                msg_box.render(out)?;
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

/// Render detailed help for a single command using StatusBox
fn render_command_details(out: &mut dyn Write, entry: &CommandEntry) -> io::Result<()> {
    let mut box_builder = StatusBox::new(format!("Command: {}", entry.name))
        .with_cols(1)
        .field("Description", &entry.description)
        .field_styled("Usage", entry.usage, FieldStyle::Accent);

    if !entry.aliases.is_empty() {
        box_builder = box_builder.field("Aliases", entry.aliases.join(", "));
    }

    // Add details section if available
    if !entry.details.is_empty() {
        box_builder = box_builder.section();
        for detail in entry.details {
            box_builder = box_builder.field_styled("", *detail, FieldStyle::Muted);
        }
    }

    box_builder.render(out)
}
