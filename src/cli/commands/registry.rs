use crate::audit::AuditActor;
use crate::config::AppConfig;
use crate::services::AppServices;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

#[async_trait]
pub trait CommandOutput: Send + Sync + 'static {
    fn push(&self, text: &str);

    /// Flush all pending output (for async implementations)
    async fn flush_all(&self) {
        // Default: no-op for sync implementations
    }
}

pub trait ConfirmationHandler: Send {
    fn handle(
        self: Box<Self>,
        accepted: bool,
        deps: &CliDependencies,
        out: &mut dyn Write,
    ) -> io::Result<CommandOutcome>;
}

pub struct ConfirmationRequest {
    prompt: String,
    handler: Box<dyn ConfirmationHandler>,
    command: Option<(String, Vec<String>)>,
}

impl ConfirmationRequest {
    pub fn new(prompt: impl Into<String>, handler: Box<dyn ConfirmationHandler>) -> Self {
        Self {
            prompt: prompt.into(),
            handler,
            command: None,
        }
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn with_command_context(mut self, name: impl Into<String>, args: &[&str]) -> Self {
        let command = name.into();
        let arguments = args.iter().map(|arg| (*arg).to_string()).collect();
        self.command = Some((command, arguments));
        self
    }

    pub fn command_context(&self) -> Option<(String, Vec<String>)> {
        self.command
            .as_ref()
            .map(|(name, args)| (name.clone(), args.clone()))
    }

    pub fn resolve(
        self,
        accepted: bool,
        deps: &CliDependencies,
        out: &mut dyn Write,
    ) -> io::Result<CommandOutcome> {
        self.handler.handle(accepted, deps, out)
    }
}

pub fn parse_confirmation_answer(input: &str) -> Option<bool> {
    let normalized = input.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "y" | "yes" | "j" | "ja" => Some(true),
        "n" | "no" | "nein" => Some(false),
        _ => None,
    }
}

pub type CommandHandler = fn(
    &CliDependencies,
    &[&str],
    &CommandRegistry,
    &mut dyn Write,
    ShellEnvironment,
) -> io::Result<CommandOutcome>;

pub type CompletionFn = fn(&CliDependencies, &CompletionContext<'_>) -> Vec<String>;

#[derive(Debug, Clone, Copy)]
pub struct CompletionContext<'a> {
    pub tokens: &'a [&'a str],
    pub active_index: usize,
    pub prefix: &'a str,
    pub environment: ShellEnvironment,
}

#[derive(Debug, Clone, Copy)]
pub enum CompletionKind {
    None,
    Static(&'static [&'static str]),
    Dynamic(CompletionFn),
}

#[derive(Debug, Clone, Copy)]
pub struct CommandArgument {
    pub name: &'static str,
    pub optional: bool,
    pub variadic: bool,
    pub completion: CompletionKind,
}

impl CommandArgument {
    pub const fn required(name: &'static str) -> Self {
        Self {
            name,
            optional: false,
            variadic: false,
            completion: CompletionKind::None,
        }
    }

    pub const fn optional(name: &'static str) -> Self {
        Self {
            name,
            optional: true,
            variadic: false,
            completion: CompletionKind::None,
        }
    }

    pub const fn with_completion(mut self, completion: CompletionKind) -> Self {
        self.completion = completion;
        self
    }

    pub const fn variadic(mut self) -> Self {
        self.variadic = true;
        self
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CommandSubcommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub arguments: &'static [CommandArgument],
    pub description: &'static str,
}

impl CommandSubcommand {
    pub const fn new(
        name: &'static str,
        aliases: &'static [&'static str],
        arguments: &'static [CommandArgument],
        description: &'static str,
    ) -> Self {
        Self {
            name,
            aliases,
            arguments,
            description,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CommandShape {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub arguments: &'static [CommandArgument],
    pub subcommands: &'static [CommandSubcommand],
}

impl CommandShape {
    pub const fn new(
        name: &'static str,
        aliases: &'static [&'static str],
        arguments: &'static [CommandArgument],
        subcommands: &'static [CommandSubcommand],
    ) -> Self {
        Self {
            name,
            aliases,
            arguments,
            subcommands,
        }
    }

    pub const fn basic(name: &'static str) -> Self {
        Self {
            name,
            aliases: &[],
            arguments: &[],
            subcommands: &[],
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub usage: &'static str,
    pub details: &'static [&'static str],
    pub handler: CommandHandler,
    pub shape: CommandShape,
}

impl CommandEntry {
    pub fn new(
        name: &'static str,
        description: &'static str,
        usage: &'static str,
        details: &'static [&'static str],
        handler: CommandHandler,
    ) -> Self {
        let shape = CommandShape::basic(name);
        Self::with_shape(name, description, usage, details, handler, shape)
    }

    pub fn with_shape(
        name: &'static str,
        description: &'static str,
        usage: &'static str,
        details: &'static [&'static str],
        handler: CommandHandler,
        shape: CommandShape,
    ) -> Self {
        Self {
            name: name.to_string(),
            aliases: shape
                .aliases
                .iter()
                .map(|alias| alias.to_string())
                .collect(),
            description: description.to_string(),
            usage,
            details,
            handler,
            shape,
        }
    }
}

pub enum CommandOutcome {
    Continue,
    ExitShell,
    EnterDbShell,
    AwaitConfirmation(ConfirmationRequest),
    AsyncTask(Pin<Box<dyn Future<Output = io::Result<CommandOutcome>> + Send>>),
}

impl fmt::Debug for CommandOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandOutcome::Continue => f.write_str("Continue"),
            CommandOutcome::ExitShell => f.write_str("ExitShell"),
            CommandOutcome::EnterDbShell => f.write_str("EnterDbShell"),
            CommandOutcome::AwaitConfirmation(_) => f.write_str("AwaitConfirmation"),
            CommandOutcome::AsyncTask(_) => f.write_str("AsyncTask"),
        }
    }
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
    alias_index: BTreeMap<String, String>,
    order: Vec<String>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: BTreeMap::new(),
            alias_index: BTreeMap::new(),
            order: Vec::new(),
        }
    }

    pub fn register(&mut self, entry: CommandEntry) {
        let canonical = entry.name.clone();
        for alias in &entry.aliases {
            self.alias_index
                .insert(alias.to_string(), canonical.clone());
        }
        if self.commands.insert(canonical.clone(), entry).is_none() {
            self.order.push(canonical);
        }
    }

    pub fn get(&self, name: &str) -> Option<&CommandEntry> {
        if let Some(entry) = self.commands.get(name) {
            return Some(entry);
        }
        if let Some(canonical) = self.alias_index.get(name) {
            return self.commands.get(canonical);
        }
        None
    }

    pub fn entries(&self) -> impl Iterator<Item = &CommandEntry> {
        self.commands.values()
    }

    pub fn command_names(&self) -> Vec<String> {
        self.order.clone()
    }

    pub fn shapes(&self) -> Vec<CommandShape> {
        self.order
            .iter()
            .filter_map(|name| self.commands.get(name))
            .map(|entry| entry.shape)
            .collect()
    }

    pub fn execute(
        &self,
        name: &str,
        args: &[&str],
        deps: &CliDependencies,
        out: &mut dyn Write,
        env: ShellEnvironment,
    ) -> io::Result<CommandStatus> {
        let started_at = Instant::now();
        let result = if let Some(entry) = self.get(name) {
            (entry.handler)(deps, args, self, out, env).map(CommandStatus::Executed)
        } else {
            Ok(CommandStatus::NotFound)
        };
        let success = result
            .as_ref()
            .map(|status| matches!(status, CommandStatus::Executed(_)))
            .unwrap_or(false);
        let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        deps.services
            .diagnostics()
            .record_probe("cli-shell", latency_ms, success);
        result
    }

    /// Find commands similar to the given input (for "Did you mean?" suggestions)
    pub fn find_similar(&self, input: &str) -> Vec<String> {
        let input_lower = input.to_lowercase();
        let mut candidates: Vec<(String, usize)> = Vec::new();

        // Collect all command names and aliases
        for entry in self.commands.values() {
            let name_lower = entry.name.to_lowercase();

            // Check prefix match
            if name_lower.starts_with(&input_lower) || input_lower.starts_with(&name_lower) {
                candidates.push((entry.name.clone(), 0));
                continue;
            }

            // Calculate edit distance
            let dist = levenshtein(&input_lower, &name_lower);
            if dist <= 3 {
                candidates.push((entry.name.clone(), dist));
            }

            // Also check aliases
            for alias in &entry.aliases {
                let alias_lower = alias.to_lowercase();
                if alias_lower.starts_with(&input_lower) || input_lower.starts_with(&alias_lower) {
                    if !candidates.iter().any(|(n, _)| n == &entry.name) {
                        candidates.push((entry.name.clone(), 0));
                    }
                } else {
                    let dist = levenshtein(&input_lower, &alias_lower);
                    if dist <= 2 && !candidates.iter().any(|(n, _)| n == &entry.name) {
                        candidates.push((entry.name.clone(), dist));
                    }
                }
            }
        }

        // Sort by distance, then alphabetically
        candidates.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

        // Return top 3 suggestions
        candidates
            .into_iter()
            .take(3)
            .map(|(name, _)| name)
            .collect()
    }
}

/// Simple Levenshtein distance for typo detection
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut matrix = vec![vec![0usize; b_len + 1]; a_len + 1];

    for (i, row) in matrix.iter_mut().enumerate().take(a_len + 1) {
        row[0] = i;
    }
    for j in 0..=b_len {
        matrix[0][j] = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }

    matrix[a_len][b_len]
}

#[derive(Clone)]
pub struct CliDependencies {
    pub config: Arc<AppConfig>,
    pub services: Arc<AppServices>,
    output: Option<Arc<dyn CommandOutput>>,
    session_actor: Option<AuditActor>,
}

impl CliDependencies {
    pub fn new(config: Arc<AppConfig>, services: Arc<AppServices>) -> Self {
        Self {
            config,
            services,
            output: None,
            session_actor: None,
        }
    }

    pub fn with_output(&self, output: Arc<dyn CommandOutput>) -> Self {
        Self {
            config: Arc::clone(&self.config),
            services: Arc::clone(&self.services),
            output: Some(output),
            session_actor: self.session_actor.clone(),
        }
    }

    pub fn with_actor(&self, actor: AuditActor) -> Self {
        Self {
            config: Arc::clone(&self.config),
            services: Arc::clone(&self.services),
            output: self.output.clone(),
            session_actor: Some(actor),
        }
    }

    pub fn output(&self) -> Option<Arc<dyn CommandOutput>> {
        self.output.clone()
    }

    pub fn session_actor(&self) -> Option<&AuditActor> {
        self.session_actor.as_ref()
    }
}
