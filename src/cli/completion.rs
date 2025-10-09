use crate::cli::commands::registry::{
    CliDependencies, CommandArgument, CommandShape, CommandSubcommand, CompletionContext,
    CompletionKind, ShellEnvironment,
};
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result as RustylineResult};
use std::sync::Mutex;
use tracing::debug;

struct CompletionState<'a> {
    tokens: Vec<&'a str>,
    prefix: &'a str,
    token_start: usize,
    active_index: usize,
}

#[derive(Clone, PartialEq, Eq)]
struct CycleKey {
    parts: Vec<String>,
}

impl CycleKey {
    fn new(state: &CompletionState<'_>) -> Self {
        let mut parts = Vec::with_capacity(state.tokens.len() + 1);
        for token in &state.tokens {
            parts.push(token.to_string());
        }
        parts.push(format!("#{}", state.active_index));
        Self { parts }
    }
}

struct CycleState {
    key: CycleKey,
    base_prefix: String,
    suggestions: Vec<String>,
    index: usize,
}

impl CycleState {
    fn new(state: &CompletionState<'_>, suggestions: Vec<String>) -> Self {
        Self {
            key: CycleKey::new(state),
            base_prefix: state.prefix.to_string(),
            suggestions,
            index: 0,
        }
    }

    fn current(&self) -> &str {
        &self.suggestions[self.index]
    }

    fn matches_base(&self, state: &CompletionState<'_>) -> bool {
        self.key == CycleKey::new(state) && state.prefix == self.base_prefix
    }

    fn matches_current(&self, state: &CompletionState<'_>) -> bool {
        self.key == CycleKey::new(state) && state.prefix == self.current()
    }
}

struct SubcommandScope<'a> {
    sub: &'a CommandSubcommand,
    command_offset: usize,
}

pub struct ContextualCompleter {
    deps: CliDependencies,
    environment: ShellEnvironment,
    shapes: Vec<CommandShape>,
    cycle_state: Mutex<Option<CycleState>>,
}

impl ContextualCompleter {
    pub fn new(
        shapes: Vec<CommandShape>,
        deps: CliDependencies,
        environment: ShellEnvironment,
    ) -> Self {
        debug!(
            target = "cli::completion",
            command_count = shapes.len(),
            ?environment,
            "initialising contextual completer"
        );
        Self {
            deps,
            environment,
            shapes,
            cycle_state: Mutex::new(None),
        }
    }

    pub fn update_catalog(&mut self, shapes: Vec<CommandShape>) {
        self.shapes = shapes;
        if let Ok(mut guard) = self.cycle_state.lock() {
            *guard = None;
        }
    }

    pub fn suggestions_for(&self, line: &str, pos: usize) -> (usize, Vec<String>) {
        let state = parse_state(line, pos);
        let suggestions = self.suggest(&state);
        (state.token_start, suggestions)
    }

    pub fn cycle_suggestions(&self, line: &str, pos: usize) -> (usize, Vec<String>) {
        let state = parse_state(line, pos);
        let suggestions = self.suggest(&state);
        let matches = self.apply_cycle(&state, suggestions);
        (state.token_start, matches)
    }

    fn command_matches(&self, prefix: &str) -> Vec<String> {
        let mut results = Vec::new();
        for shape in &self.shapes {
            if prefix.is_empty() || shape.name.starts_with(prefix) {
                results.push(shape.name.to_string());
            }
            for alias in shape.aliases {
                if prefix.is_empty() || alias.starts_with(prefix) {
                    if !results.iter().any(|entry| entry == alias) {
                        results.push((*alias).to_string());
                    }
                }
            }
        }
        results
    }

    fn suggest(&self, state: &CompletionState<'_>) -> Vec<String> {
        debug!(
            target = "cli::completion",
            tokens = ?state.tokens,
            prefix = state.prefix,
            active = state.active_index,
            "completion request"
        );
        if state.tokens.is_empty() {
            if !state.prefix.is_empty() {
                if let Some(shape) = self.find_shape(state.prefix) {
                    if !shape.subcommands.is_empty() {
                        debug!(
                            target = "cli::completion",
                            command = shape.name,
                            "direct match for prefix without tokens"
                        );
                        return self.subcommand_matches(shape, "");
                    }
                }
            }

            let matches = self.command_matches(state.prefix);
            if matches.len() == 1 {
                if let Some(shape) = self.find_shape(matches[0].as_str()) {
                    if !shape.subcommands.is_empty() {
                        debug!(
                            target = "cli::completion",
                            command = shape.name,
                            "single global match with subcommands"
                        );
                        return self.subcommand_matches(shape, "");
                    }
                }
            }

            return matches;
        }

        let command_token = state.tokens[0];
        if let Some(shape) = self.find_shape(command_token) {
            let suggestions = self.suggest_from_shape(shape, state);
            if !suggestions.is_empty() {
                debug!(
                    target = "cli::completion",
                    command = shape.name,
                    count = suggestions.len(),
                    "shape-based suggestions"
                );
                return suggestions;
            }

            if !shape.subcommands.is_empty() && state.active_index >= 1 {
                debug!(
                    target = "cli::completion",
                    command = shape.name,
                    "fallback to subcommand list"
                );
                return self.subcommand_matches(shape, state.prefix);
            }

            return self.complete_arguments(
                shape.arguments,
                state.active_index.saturating_sub(1),
                state,
            );
        }

        if state.tokens.len() == 1 {
            let matches = self.command_matches(state.prefix);
            if matches.len() == 1 {
                if let Some(shape) = self.find_shape(matches[0].as_str()) {
                    if !shape.subcommands.is_empty() {
                        debug!(
                            target = "cli::completion",
                            command = shape.name,
                            "single-part command completing to subcommands"
                        );
                        return self.subcommand_matches(shape, "");
                    }
                }
            }
            matches
        } else {
            Vec::new()
        }
    }

    fn suggest_from_shape(&self, shape: &CommandShape, state: &CompletionState<'_>) -> Vec<String> {
        if state.active_index == 1 && !shape.subcommands.is_empty() {
            return self.subcommand_matches(shape, state.prefix);
        }

        if let Some(scope) = self.detect_subcommand_scope(shape, state) {
            if state.active_index <= scope.command_offset - 1 {
                return self.subcommand_matches(shape, state.prefix);
            }
            return self.complete_arguments(
                scope.sub.arguments,
                state.active_index - scope.command_offset,
                state,
            );
        }

        if !shape.subcommands.is_empty() && state.tokens.len() >= 2 {
            return self.subcommand_matches(shape, state.prefix);
        }

        self.complete_arguments(shape.arguments, state.active_index.saturating_sub(1), state)
    }

    fn subcommand_matches(&self, shape: &CommandShape, prefix: &str) -> Vec<String> {
        let mut suggestions = Vec::new();
        for sub in shape.subcommands {
            if prefix.is_empty() || sub.name.starts_with(prefix) {
                suggestions.push(sub.name.to_string());
            }
            for alias in sub.aliases {
                if prefix.is_empty() || alias.starts_with(prefix) {
                    if !suggestions.iter().any(|entry| entry == alias) {
                        suggestions.push((*alias).to_string());
                    }
                }
            }
        }
        suggestions
    }

    fn detect_subcommand_scope<'a>(
        &self,
        shape: &'a CommandShape,
        state: &CompletionState<'_>,
    ) -> Option<SubcommandScope<'a>> {
        if shape.subcommands.is_empty() || state.tokens.len() < 2 {
            return None;
        }
        let candidate = state.tokens[1];
        let sub = self.find_subcommand(shape, candidate)?;
        Some(SubcommandScope {
            sub,
            command_offset: 2,
        })
    }

    fn find_shape(&self, token: &str) -> Option<&CommandShape> {
        self.shapes
            .iter()
            .find(|shape| shape.name == token || shape.aliases.iter().any(|alias| *alias == token))
    }

    fn find_subcommand<'a>(
        &self,
        shape: &'a CommandShape,
        token: &str,
    ) -> Option<&'a CommandSubcommand> {
        shape
            .subcommands
            .iter()
            .find(|sub| sub.name == token || sub.aliases.iter().any(|alias| *alias == token))
    }

    fn complete_arguments(
        &self,
        arguments: &'static [CommandArgument],
        scope_index: usize,
        state: &CompletionState<'_>,
    ) -> Vec<String> {
        if arguments.is_empty() {
            return Vec::new();
        }

        let target = if scope_index >= arguments.len() {
            arguments.last().filter(|arg| arg.variadic).copied()
        } else {
            arguments.get(scope_index).copied()
        };

        let Some(argument) = target else {
            return Vec::new();
        };

        self.resolve_completion(argument, scope_index, state)
    }

    fn resolve_completion(
        &self,
        argument: CommandArgument,
        scope_index: usize,
        state: &CompletionState<'_>,
    ) -> Vec<String> {
        match argument.completion {
            CompletionKind::None => Vec::new(),
            CompletionKind::Static(values) => values
                .iter()
                .copied()
                .filter(|value| state.prefix.is_empty() || value.starts_with(state.prefix))
                .map(|value| value.to_string())
                .collect(),
            CompletionKind::Dynamic(func) => {
                let context = CompletionContext {
                    tokens: state.tokens.as_slice(),
                    active_index: scope_index,
                    prefix: state.prefix,
                    environment: self.environment,
                };
                let results = func(&self.deps, &context);
                if state.prefix.is_empty() {
                    results
                } else {
                    results
                        .into_iter()
                        .filter(|value| value.starts_with(state.prefix))
                        .collect()
                }
            }
        }
    }

    fn apply_cycle(&self, state: &CompletionState<'_>, suggestions: Vec<String>) -> Vec<String> {
        if suggestions.is_empty() {
            if let Ok(mut guard) = self.cycle_state.lock() {
                *guard = None;
            }
            return suggestions;
        }

        let mut guard = self.cycle_state.lock().expect("cycle state mutex poisoned");
        let suggestions = suggestions;

        if let Some(existing) = guard.as_mut() {
            if existing.matches_base(state) {
                if existing.suggestions != suggestions {
                    existing.suggestions = suggestions.clone();
                    existing.index = 0;
                    return vec![existing.current().to_string()];
                }
            }

            if existing.matches_current(state) {
                if !existing.suggestions.is_empty() {
                    existing.index = (existing.index + 1) % existing.suggestions.len();
                    return vec![existing.current().to_string()];
                }
            }
        }

        let cycle = CycleState::new(state, suggestions);
        let first = cycle.current().to_string();
        *guard = Some(cycle);
        vec![first]
    }
}

impl Helper for ContextualCompleter {}

impl Completer for ContextualCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> RustylineResult<(usize, Vec<Pair>)> {
        let state = parse_state(line, pos);
        let suggestions = self.suggest(&state);
        let matches = self.apply_cycle(&state, suggestions);

        let pairs = matches
            .into_iter()
            .map(|m| Pair {
                display: m.clone(),
                replacement: m,
            })
            .collect();
        Ok((state.token_start, pairs))
    }
}

impl Hinter for ContextualCompleter {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        None
    }
}

impl Highlighter for ContextualCompleter {}
impl Validator for ContextualCompleter {}

fn parse_state<'a>(line: &'a str, pos: usize) -> CompletionState<'a> {
    let pos = pos.min(line.len());
    let head = &line[..pos];
    let has_trailing = head
        .chars()
        .last()
        .map(|c| c.is_whitespace())
        .unwrap_or(false);

    let mut tokens: Vec<&str> = head.split_whitespace().collect();
    let prefix = if has_trailing {
        ""
    } else if let Some(last) = tokens.pop() {
        last
    } else {
        ""
    };

    let token_start = pos.saturating_sub(prefix.len());

    let active_index = tokens.len();
    CompletionState {
        tokens,
        prefix,
        token_start,
        active_index,
    }
}

/// Lightweight completer for static token lists (DB shell, etc.).
pub struct ListCompleter {
    entries: Vec<String>,
}

impl ListCompleter {
    pub fn new(entries: Vec<String>) -> Self {
        Self { entries }
    }

    pub fn update(&mut self, entries: Vec<String>) {
        self.entries = entries;
    }
}

impl Helper for ListCompleter {}

impl Completer for ListCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> RustylineResult<(usize, Vec<Pair>)> {
        let (start, matches) = completion_matches(&self.entries, line, pos);
        let pairs = matches
            .into_iter()
            .map(|m| Pair {
                display: m.clone(),
                replacement: m,
            })
            .collect();
        Ok((start, pairs))
    }
}

impl Hinter for ListCompleter {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        None
    }
}

impl Highlighter for ListCompleter {}
impl Validator for ListCompleter {}

/// Compute matches for the current token in `line` at position `pos`.
pub fn completion_matches(entries: &[String], line: &str, pos: usize) -> (usize, Vec<String>) {
    let pos = pos.min(line.len());
    let start = line[..pos]
        .rfind(|c: char| c.is_whitespace())
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let prefix = &line[start..pos];
    let matches = entries
        .iter()
        .filter(|entry| entry.starts_with(prefix))
        .cloned()
        .collect();
    (start, matches)
}

/// Determine the longest common prefix among all matches.
pub fn longest_common_prefix(strings: &[String]) -> Option<String> {
    let mut iter = strings.iter();
    let mut prefix = iter.next()?.clone();
    for s in iter {
        while !s.starts_with(&prefix) {
            if prefix.is_empty() {
                return Some(String::new());
            }
            prefix.pop();
        }
    }
    Some(prefix)
}

#[cfg(test)]
mod tests {
    use super::{parse_state, ContextualCompleter};
    use crate::audit::InMemoryAuditLog;
    use crate::cli::commands::builtins;
    use crate::cli::commands::registry::{CliDependencies, ShellEnvironment};
    use crate::config::{
        AppConfig, AppSection, AuditSection, CliSection, DbConnectionSettings, DbConnections,
        DbPoolSettings, DbSection, HttpConfig, HttpSecuritySection, HttpTlsConfig, JwtConfig,
        KdfConfig, ModuleRegistrySection, ModuleStorageSection, ModuleTrustSection, ModulesSection,
        SecuritySection, ServerSection, SshConfig, SshTlsConfig, TelemetrySection,
    };
    use crate::domain::db::{
        DbAdminPort, DbEngine, DbExecutionResult, DbResult, DbTable, DbTableSchema,
    };
    use crate::infra::storage::memory::{InMemoryTicketRepository, InMemoryUserRepository};
    use crate::services::ServiceRegistry;
    use crate::services::{
        AppServices, DbShellService, SchedulerService, TicketService, UserService,
    };
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct DummyDbAdapter;

    #[async_trait]
    impl DbAdminPort for DummyDbAdapter {
        async fn ping(&self) -> DbResult<()> {
            Ok(())
        }

        async fn simple_query(&self, _statement: &str) -> DbResult<Vec<DbExecutionResult>> {
            Err(crate::domain::db::DbError::NotImplemented {
                message: "simple_query not available in DummyDbAdapter".to_string(),
            })
        }

        async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
            Ok(Vec::new())
        }

        async fn describe_table(&self, _table: &str) -> DbResult<DbTableSchema> {
            Err(crate::domain::db::DbError::NotImplemented {
                message: "describe_table not available in DummyDbAdapter".to_string(),
            })
        }
    }

    fn test_config() -> Arc<AppConfig> {
        let mut connections = DbConnections::default();
        connections.postgres = Some(DbConnectionSettings {
            uri: "postgres://test".to_string(),
            pool: DbPoolSettings {
                max: Some(8),
                timeout_ms: Some(1000),
            },
        });

        Arc::new(AppConfig {
            app: AppSection {
                name: "fenrir-test".to_string(),
                version: "0.0.0".to_string(),
            },
            server: ServerSection {
                enable_http: false,
                enable_grpc: false,
                ssh: SshConfig {
                    host: "127.0.0.1".to_string(),
                    port: 2222,
                    user: "tester".to_string(),
                    server_name: "fenrir-test".to_string(),
                    host_key_path: "keys/test_ed25519".to_string(),
                    idle_close_seconds: Some(30),
                    tls: SshTlsConfig::default(),
                },
                http: HttpConfig {
                    host: "127.0.0.1".to_string(),
                    port: 8080,
                    tls: HttpTlsConfig::default(),
                },
            },
            security: SecuritySection {
                kdf: KdfConfig {
                    algorithm: "argon2id".to_string(),
                },
                jwt: JwtConfig {
                    issuer: "fenrir".to_string(),
                    audience: "fenrir".to_string(),
                    exp_seconds: 3600,
                },
                allowed_ciphers: vec!["AES-GCM".to_string()],
                http: HttpSecuritySection {
                    control_tokens: Vec::new(),
                },
            },
            db: DbSection {
                default_engine: "postgres".to_string(),
                connections,
            },
            telemetry: TelemetrySection {
                tracing_level: "info".to_string(),
                metrics_enabled: false,
                health_enabled: false,
            },
            audit: AuditSection { enabled: false },
            cli: CliSection {
                prompt_theme: "default".to_string(),
            },
            modules: ModulesSection {
                registry: ModuleRegistrySection {
                    url: "http://localhost:3001".to_string(),
                    allow_offline: true,
                    auth_token: None,
                },
                storage: ModuleStorageSection {
                    install_dir: "tmp/test-modules".to_string(),
                    cache_dir: Some("tmp/test-modules/cache".to_string()),
                },
                trust: ModuleTrustSection::default(),
            },
        })
    }

    fn test_dependencies() -> CliDependencies {
        let config = test_config();
        let registry = Arc::new(ServiceRegistry::new());
        let scheduler = Arc::new(SchedulerService::new(Arc::clone(&registry)));

        let mut adapters: BTreeMap<DbEngine, Arc<dyn DbAdminPort>> = BTreeMap::new();
        adapters.insert(DbEngine::Postgres, Arc::new(DummyDbAdapter));
        let db_shell =
            Arc::new(DbShellService::new(DbEngine::Postgres, adapters).expect("db shell"));

        let ticket_repo = Arc::new(InMemoryTicketRepository::new());
        let ticket = Arc::new(TicketService::new(ticket_repo));
        let user_repo = Arc::new(InMemoryUserRepository::new());
        let user = Arc::new(UserService::new(user_repo));
        let audit = Arc::new(InMemoryAuditLog::new(32));

        let services = Arc::new(AppServices::new(
            Arc::clone(&db_shell),
            Arc::clone(&scheduler),
            Arc::clone(&ticket),
            Arc::clone(&user),
            Arc::clone(&registry),
            audit,
        ));

        CliDependencies::new(config, services)
    }

    #[test]
    fn parse_state_after_command_space_identifies_subcommand_slot() {
        let line = "modules ";
        let state = parse_state(line, line.len());

        assert_eq!(state.tokens, vec!["modules"]);
        assert_eq!(state.prefix, "");
        assert_eq!(state.active_index, 1);
    }

    #[test]
    fn parse_state_without_trailing_space_keeps_prefix() {
        let line = "modules lo";
        let state = parse_state(line, line.len());

        assert_eq!(state.tokens, vec!["modules"]);
        assert_eq!(state.prefix, "lo");
        assert_eq!(state.token_start, line.len() - 2);
    }

    #[test]
    fn suggestions_for_modules_command_prefer_subcommands() {
        let dependencies = test_dependencies();
        let registry = builtins::build_registry();
        let shapes = registry.shapes();
        let completer = ContextualCompleter::new(shapes, dependencies, ShellEnvironment::Cli);

        let modules_entry = registry.get("modules").expect("modules command registered");
        let mut expected = std::collections::BTreeSet::new();
        for sub in modules_entry.shape.subcommands {
            expected.insert(sub.name.to_string());
            for alias in sub.aliases {
                expected.insert((*alias).to_string());
            }
        }

        let (_, suggestions) = completer.suggestions_for("modules", "modules".len());
        assert!(
            !suggestions.is_empty(),
            "expected suggestions for modules command"
        );
        for suggestion in &suggestions {
            assert!(
                expected.contains(suggestion),
                "unexpected suggestion '{suggestion}' for modules command",
            );
        }
    }
}
