use super::registry::CommandRegistry;

pub mod db_shell;
pub mod exit;
pub mod help;
pub mod log;
pub mod modules;
pub mod services;
pub mod ticket;
pub mod user;

pub fn register_builtins(registry: &mut CommandRegistry) {
    registry.register(help::command());
    registry.register(db_shell::command());
    registry.register(services::command());
    registry.register(log::command());
    registry.register(modules::command());
    registry.register(user::command());
    registry.register(ticket::command());
    registry.register(exit::command());
}

pub fn build_registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);
    registry
}
