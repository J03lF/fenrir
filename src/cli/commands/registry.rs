use crate::config::AppConfig;
use crate::services::AppServices;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::Arc;

pub type CommandHandler = fn(
    &CliDependencies,
    &[&str],
    &CommandRegistry,
    &mut dyn Write,
    ShellEnvironment,
) -> io::Result<CommandOutcome>;

#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub name: String,
    pub description: String,
    pub handler: CommandHandler,
}

impl CommandEntry {
    pub fn new(name: &'static str, description: &'static str, handler: CommandHandler) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            handler,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
    Continue,
    ExitShell,
    EnterDbShell,
}

pub enum CommandStatus {
    Executed(CommandOutcome),
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellEnvironment {
    Cli,
    Ssh,
}

#[derive(Clone)]
pub struct CommandRegistry {
    commands: BTreeMap<String, CommandEntry>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, entry: CommandEntry) {
        self.commands.insert(entry.name.clone(), entry);
    }

    pub fn get(&self, name: &str) -> Option<&CommandEntry> {
        self.commands.get(name)
    }

    pub fn entries(&self) -> impl Iterator<Item = &CommandEntry> {
        self.commands.values()
    }

    pub fn command_names(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }

    pub fn execute(
        &self,
        name: &str,
        args: &[&str],
        deps: &CliDependencies,
        out: &mut dyn Write,
        env: ShellEnvironment,
    ) -> io::Result<CommandStatus> {
        if let Some(entry) = self.get(name) {
            (entry.handler)(deps, args, self, out, env).map(CommandStatus::Executed)
        } else {
            Ok(CommandStatus::NotFound)
        }
    }
}

#[derive(Clone)]
pub struct CliDependencies {
    pub config: Arc<AppConfig>,
    pub services: Arc<AppServices>,
}

impl CliDependencies {
    pub fn new(config: Arc<AppConfig>, services: Arc<AppServices>) -> Self {
        Self { config, services }
    }
}
