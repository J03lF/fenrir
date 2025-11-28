use crate::cli::commands::registry::CommandRegistry;

use super::{audit, clear, db_shell, exit, help, log, modules, services, show, user};

pub fn register_builtins(registry: &mut CommandRegistry) {
    registry.register(help::command());
    registry.register(clear::command());
    registry.register(audit::command());
    registry.register(db_shell::command());
    registry.register(services::list_command());
    registry.register(services::start_command());
    registry.register(services::stop_command());
    registry.register(services::restart_command());
    registry.register(log::command());
    registry.register(modules::search_command());
    registry.register(modules::install_command());
    registry.register(modules::synchronize_command());
    registry.register(modules::release_command());
    registry.register(modules::uninstall_command());
    registry.register(modules::check_command());
    registry.register(modules::logs_command());
    registry.register(user::command());
    registry.register(show::command());
    registry.register(exit::command());
}

pub fn build_registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);
    registry
}
