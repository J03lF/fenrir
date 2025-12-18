use std::io::{self, Write};
use std::path::PathBuf;
use tokio::runtime::Handle;

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CommandSubcommand, CompletionKind, ShellEnvironment,
};

use crate::cli::commands::builtins::db_schema::export::parse_engine;
use crate::cli::commands::builtins::db_schema::import::run_import;

const SCHEMA_ARGS: &[CommandArgument] = &[
    CommandArgument::required("file"),
    CommandArgument::optional("engine")
        .with_completion(CompletionKind::Static(&["postgres", "sqlite"])),
    CommandArgument::optional("--dry-run"),
    CommandArgument::optional("--force"),
];

const IMPORT_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new(
        "schema",
        &["db"],
        SCHEMA_ARGS,
        "Import StarUML schema and apply migrations",
    ),
];

const IMPORT_SHAPE: CommandShape = CommandShape::new("import", &[], &[], IMPORT_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "import",
        "Import resources",
        "import <resource> <file> [options]",
        &["Import schema, data, or other resources"],
        handle,
        IMPORT_SHAPE,
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
                "usage: import schema <file> [engine] [--dry-run] [--force]".to_string()
            } else {
                let path = PathBuf::from(tail[0]);
                let engine = parse_engine(tail.get(1).filter(|v| !v.starts_with("--")).copied());
                let dry_run = tail.contains(&"--dry-run");
                let force = tail.contains(&"--force");
                let services = deps.services.as_ref();

                let result = block_on_result(run_import(services, path, engine, dry_run, force));
                match result {
                    Ok(msg) => msg,
                    Err(err) => format!("error: {err}"),
                }
            }
        }
        other => format!("unknown import target: {other}\nvalid: schema"),
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

