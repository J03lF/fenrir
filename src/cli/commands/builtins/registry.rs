use crate::cli::commands::registry::CommandRegistry;

use super::{audit, backup, clear, db_shell, exit, export, help, import, log, modules, restore, services, show, status, user};

pub fn register_builtins(registry: &mut CommandRegistry) {
    // Core commands
    registry.register(help::command());
    registry.register(clear::command());
    registry.register(exit::command());
    
    // Verb-first commands (export/import/backup/restore/log/status)
    registry.register(export::command());
    registry.register(import::command());
    registry.register(backup::command());
    registry.register(restore::command());
    registry.register(log::command());
    registry.register(status::command());
    registry.register(show::command());
    
    // Service control (start/stop/restart/pause/resume)
    registry.register(services::list_command());
    registry.register(services::start_command());
    registry.register(services::stop_command());
    registry.register(services::restart_command());
    registry.register(services::pause_command());
    registry.register(services::resume_command());
    
    // Module commands
    registry.register(modules::search_command());
    registry.register(modules::install_command());
    registry.register(modules::synchronize_command());
    registry.register(modules::release_command());
    registry.register(modules::scaffold_command());
    registry.register(modules::uninstall_command());
    registry.register(modules::check_command());
    
    // Other
    registry.register(audit::command());
    registry.register(user::command());
    registry.register(db_shell::command());  // db: subshell
}

pub fn build_registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);
    registry
}
