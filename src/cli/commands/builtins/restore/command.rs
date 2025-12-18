use std::io::{self, Write};
use tokio::runtime::Handle;

use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandEntry, CommandOutcome, CommandRegistry, CommandShape,
    CommandSubcommand, ShellEnvironment,
};

const DB_ARGS: &[CommandArgument] = &[
    CommandArgument::required("backup_file"),
];

const RESTORE_SUBCOMMANDS: &[CommandSubcommand] = &[
    CommandSubcommand::new(
        "db",
        &["database"],
        DB_ARGS,
        "Restore embedded database from backup",
    ),
];

const RESTORE_SHAPE: CommandShape = CommandShape::new("restore", &[], &[], RESTORE_SUBCOMMANDS);

pub fn command() -> CommandEntry {
    CommandEntry::with_shape(
        "restore",
        "Restore resources",
        "restore <resource> <backup_file>",
        &["Restore db, config, or other resources from backup"],
        handle,
        RESTORE_SHAPE,
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
            let Some(backup_file) = tail.first() else {
                return writeln!(out, "usage: restore db <backup_file>")
                    .map(|_| CommandOutcome::Continue);
            };

            let Some(rt) = deps.services.db_runtime() else {
                return writeln!(out, "db-runtime not available (mode != embedded?)")
                    .map(|_| CommandOutcome::Continue);
            };

            let result = block_on_result(rt.restore(backup_file));
            match result {
                Ok(()) => format!("restore completed from: {backup_file}"),
                Err(err) => format!("restore failed: {err}"),
            }
        }
        other => format!("unknown restore target: {other}\nvalid: db"),
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

