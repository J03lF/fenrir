use std::io::{self, Write};
use tokio::runtime::Handle;

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CommandSubcommand, ShellEnvironment,
};

const DB_ARGS: &[CommandArgument] = &[
    CommandArgument::optional("label"),
];

const BACKUP_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new(
        "db",
        &["database"],
        DB_ARGS,
        "Backup embedded database",
    ),
];

const BACKUP_SHAPE: CommandShape = CommandShape::new("backup", &[], &[], BACKUP_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "backup",
        "Backup resources",
        "backup <resource> [options]",
        &["Create backups of db, config, or other resources"],
        handle,
        BACKUP_SHAPE,
    )
}

fn handle(
    deps: &CliDependencies,
    args: &[&str],
    _registry: &CommandRegistry,
    out: &mut dyn Write,
    _env: ShellEnvironment,
) -> io::Result<CommandOutcome> {
    let sub = args.first().copied().unwrap_or("db");
    let tail = if args.is_empty() { args } else { &args[1..] };

    let output = match sub {
        "db" | "database" => {
            let label = tail.first().map(|s| s.to_string());
            
            let Some(rt) = deps.services.db_runtime() else {
                return writeln!(out, "db-runtime not available (mode != embedded?)")
                    .map(|_| CommandOutcome::Continue);
            };

            let result = block_on_result(rt.backup(label));
            match result {
                Ok(path) => format!("backup created: {path}"),
                Err(err) => format!("backup failed: {err}"),
            }
        }
        other => format!("unknown backup target: {other}\nvalid: db"),
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

