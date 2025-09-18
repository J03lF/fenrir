use super::registry::CommandRegistry;

pub mod db_shell;
pub mod exit;
pub mod help;

pub fn register_builtins(registry: &mut CommandRegistry) {
    registry.register(help::command());
    registry.register(db_shell::command());
    registry.register(exit::command());
}

pub fn build_registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry);
    registry
}
