use std::io::{self, Write};
use std::path::PathBuf;
use tokio::runtime::Handle;

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CompletionContext, CompletionKind, ShellEnvironment,
};

use super::export::{parse_engine, run_export, ExportMode};
use super::import::run_import;

const EXPORT_ARGS: &[CommandArgument] = &[
    CommandArgument::required("target"),  // "all", "table <name>", or file path
    CommandArgument::optional("name_or_engine"),
    CommandArgument::optional("engine")
        .with_completion(CompletionKind::Static(&["postgres", "sqlite", "mysql", "mongodb"])),
];
const IMPORT_ARGS: &[CommandArgument] = &[
    CommandArgument::required("file"),
    CommandArgument::optional("engine")
        .with_completion(CompletionKind::Static(&["postgres", "sqlite", "mysql", "mongodb"])),
    CommandArgument::optional("--dry-run"),
    CommandArgument::optional("--force"),
];

pub fn command() -> CommandEntry {
    const SUBS: &[crate::cli::commands::registry::CommandSubcommand] = &[
        crate::cli::commands::registry::CommandSubcommand::new(
            "export",
            &["exp"],
            EXPORT_ARGS,
            "Export DB schema to StarUML (.mdj)",
        ),
        crate::cli::commands::registry::CommandSubcommand::new(
            "import",
            &["imp"],
            IMPORT_ARGS,
            "Import StarUML (.mdj) and apply migrations",
        ),
    ];
    let shape = CommandShape::new("db schema", &[], &[], SUBS);
    CommandEntry::with_shape(
        "db schema",
        "DB schema utilities",
        "db schema <subcommand>",
        &["Manage and export DB schema"],
        handle,
        shape,
    )
}

pub fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let sub = args.first().copied().unwrap_or("export");
    let tail = &args[1..];
    let output = match sub {
        "export" | "exp" => {
            if tail.is_empty() {
                format!(
                    "usage:\n  db schema export all              - export all tables\n  \
                     db schema export table <name>      - export specific table\n  \
                     db schema export <file> [engine]   - export to file\n\n\
                     export dir: {}", deps.config.db.schema.export_dir
                )
            } else {
                // Parse export mode from arguments
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
        "import" | "imp" => {
            if tail.is_empty() {
                "usage: db schema import <file> [engine] [--dry-run] [--force]".to_string()
            } else {
                let path = PathBuf::from(tail[0]);
                let engine = parse_engine(tail.get(1).filter(|v| !v.starts_with("--")).copied());
                let dry_run = tail.contains(&"--dry-run");
                let force = tail.contains(&"--force");
                let services = deps.services.as_ref();
                // Run async operation first, then write result
                let result = block_on_result(run_import(services, path, engine, dry_run, force));
                match result {
                    Ok(msg) => msg,
                    Err(err) => format!("error: {err}"),
                }
            }
        }
        other => format!("unknown subcommand {other}"),
    };
    writeln!(out, "{output}")?;
    Ok(CommandOutcome::Continue)
}

pub fn completion(ctx: &CompletionContext<'_>) -> Vec<String> {
    let subs = ["export", "exp", "import", "imp"];
    match ctx.active_index {
        0 => subs
            .iter()
            .filter(|s| s.starts_with(ctx.prefix))
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn block_on_result<F, T, E>(fut: F) -> Result<T, E>
where
    F: std::future::Future<Output = Result<T, E>> + Send,
    T: Send,
    E: Send,
{
    if let Ok(handle) = Handle::try_current() {
        // Use block_in_place to allow blocking within a tokio runtime
        tokio::task::block_in_place(|| handle.block_on(fut))
    } else {
        tokio::runtime::Runtime::new()
            .expect("failed to create runtime")
            .block_on(fut)
    }
}

