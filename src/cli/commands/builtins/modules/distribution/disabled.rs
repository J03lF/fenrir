//! The depot commands of a build without the `depot` feature.

use std::io::{self, Write};

use crate::cli::commands::registry::{CliDependencies, CommandOutcome};
use crate::utils::messages::cli::builtins::distribution as msg;

fn not_built(out: &mut dyn Write) -> io::Result<CommandOutcome> {
    writeln!(out, "{}", msg::depot::NOT_BUILT)?;
    Ok(CommandOutcome::Continue)
}

pub(crate) fn depot_configured() -> bool {
    false
}

pub(crate) fn install_distribution(
    _deps: &CliDependencies,
    _args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    not_built(out)
}

pub(crate) fn install_module(
    _deps: &CliDependencies,
    _args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    not_built(out)
}

pub(crate) fn rollback(
    _deps: &CliDependencies,
    _args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    not_built(out)
}

pub(crate) fn pin(
    _deps: &CliDependencies,
    _args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    not_built(out)
}

pub(crate) fn unpin(
    _deps: &CliDependencies,
    _args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    not_built(out)
}

pub(crate) fn forget_module(_deps: &CliDependencies, _module: &str) {}

pub(crate) fn status(_deps: &CliDependencies, out: &mut dyn Write) -> io::Result<CommandOutcome> {
    not_built(out)
}

pub(crate) fn search_distributions(
    _deps: &CliDependencies,
    _args: &[&str],
    out: &mut dyn Write,
) -> io::Result<CommandOutcome> {
    not_built(out)
}

pub(crate) fn search_modules(
    _deps: &CliDependencies,
    _args: &[&str],
    _out: &mut dyn Write,
) -> io::Result<Option<CommandOutcome>> {
    Ok(None)
}
