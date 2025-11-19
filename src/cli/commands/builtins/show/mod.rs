use super::modules;
use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionContext, CompletionKind, ShellEnvironment,
};
use std::io::{self, Write};

const SHOW_RESOURCE_OPTIONS: &[&str] = &["module", "modules"];
const SHOW_RESOURCE_ARGUMENT: CommandArgument = CommandArgument::required("resource")
    .with_completion(CompletionKind::Static(SHOW_RESOURCE_OPTIONS));
const SHOW_TARGET_ARGUMENT: CommandArgument = CommandArgument::required("target")
    .with_completion(CompletionKind::Dynamic(complete_show_targets));

const SHOW_ARGUMENTS: &[CommandArgument] = &[SHOW_RESOURCE_ARGUMENT, SHOW_TARGET_ARGUMENT];
const SHOW_SHAPE: CommandShape = CommandShape::new("show", &[], SHOW_ARGUMENTS, &[]);

const SHOW_DETAILS: &[&str] = &["show module <name[@version]> – zeigt Modul-Metadaten"];

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "show",
        "Zeigt Modul-Metadaten",
        "show module <name[@version]>",
        SHOW_DETAILS,
        handle_show,
        SHOW_SHAPE,
    )
}

fn handle_show(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let Some((resource, tail)) = args.split_first() else {
        writeln!(out, "Nutzung: show <user|ticket|module> <ziel>")?;
        return Ok(CommandOutcome::Continue);
    };

    match resource.to_ascii_lowercase().as_str() {
        "module" | "modules" => {
            if tail.is_empty() {
                writeln!(out, "Nutzung: show module <name[@version]>")?;
            } else {
                return modules::run_module_command(deps, "info", tail, out);
            }
        }
        other => {
            writeln!(out, "unbekannte Ressource: {other}")?;
            writeln!(out, "verfügbar: show module")?;
        }
    }

    Ok(CommandOutcome::Continue)
}

fn complete_show_targets(deps: &CliDependencies, ctx: &CompletionContext<'_>) -> Vec<String> {
    let Some(resource) = ctx.tokens.get(1).copied() else {
        return Vec::new();
    };

    if matches!(resource, "module" | "modules") {
        modules::complete_module_ids(deps, ctx)
    } else {
        Vec::new()
    }
}
