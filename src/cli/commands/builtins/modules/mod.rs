mod ctx;
mod distribution;
mod entries;
mod handlers;
mod output;
mod tasks;

pub use entries::{
    check_command, command, complete_module_ids, install_command, pin_command, release_command,
    rollback_command, scaffold_command, search_command, synchronize_command, uninstall_command,
    unpin_command,
};
pub use handlers::run_module_command;

/// `status distribution`.
pub fn distribution_status(
    deps: &crate::cli::commands::registry::CliDependencies,
    out: &mut dyn std::io::Write,
) -> std::io::Result<crate::cli::commands::registry::CommandOutcome> {
    distribution::status(deps, out)
}
