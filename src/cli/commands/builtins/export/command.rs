use std::io::{self, Write};
use tokio::runtime::Handle;

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CommandSubcommand, CompletionKind, ShellEnvironment,
};

use crate::cli::commands::builtins::db_schema::export::{parse_engine, run_export, ExportMode};

const SCHEMA_ARGS: &[CommandArgument] = &[
    CommandArgument::required("target"),  // "all", "table <name>", or file path
    CommandArgument::optional("name_or_engine"),
    CommandArgument::optional("engine")
        .with_completion(CompletionKind::Static(&["postgres", "sqlite"])),
];

const EXPORT_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new(
        "schema",
        &["db"],
        SCHEMA_ARGS,
        "Export database schema to StarUML (.mdj)",
    ),
];

const EXPORT_SHAPE: CommandShape = CommandShape::new("export", &[], &[], EXPORT_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "export",
        "Export resources",
        "export <resource> [options]",
        &["Export schema, data, or other resources"],
        handle,
        EXPORT_SHAPE,
    )
}

fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let sub = args.first().copied().unwrap_or("schema");
    let tail = if args.is_empty() { args } else { &args[1..] };

    let output = match sub {
        "schema" | "db" => {
            if tail.is_empty() {
                format!(
                    "usage:\n  export schema all              - export all tables\n  \
                     export schema table <name>      - export specific table\n  \
                     export schema <file> [engine]   - export to file\n\n\
                     export dir: {}", deps.config.db.schema.export_dir
                )
            } else {
                let mode = ExportMode::parse(tail);
                let engine = match tail.first().copied() {
                    Some("all") => parse_engine(tail.get(1).copied()),
                    Some("table") => parse_engine(tail.get(2).copied()),
                    _ => parse_engine(tail.get(1).copied()),
                };

                match mode {
                    Some(mode) => {
                        let services = deps.services.as_ref();
                        let config = &deps.config;
                        let result = block_on_result(run_export(services, config, engine, mode));
                        match result {
                            Ok(msg) => msg,
                            Err(err) => format!("error: {err}"),
                        }
                    }
                    None => "invalid export arguments".to_string(),
                }
            }
        }
        other => format!("unknown export target: {other}\nvalid: schema"),
    };

    writeln!(out, "{output}")?;
    Ok(CommandOutcome::Continue)
}

fn block_on_result<F, T, E>(fut: F) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>> + Send,
    T: Send,
    E: Send,
{
    if let Ok(handle) = Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(fut))
    } else {
        tokio::runtime::Runtime::new()
            .expect("failed to create runtime")
            .block_on(fut)
    }
}

