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
                if (prefix.is_empty() || alias.starts_with(prefix))
                    && !results.iter().any(|entry| entry == alias)
                {
                    results.push((*alias).to_string());
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
            return self.command_matches(state.prefix);
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
            if state.active_index < scope.command_offset {
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
                if (prefix.is_empty() || alias.starts_with(prefix))
                    && !suggestions.iter().any(|entry| entry == alias)
                {
                    suggestions.push((*alias).to_string());
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
            .find(|shape| shape.name == token || shape.aliases.contains(&token))
    }

    fn find_subcommand<'a>(
        &self,
        shape: &'a CommandShape,
        token: &str,
    ) -> Option<&'a CommandSubcommand> {
        shape
            .subcommands
            .iter()
            .find(|sub| sub.name == token || sub.aliases.contains(&token))
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
        let mut suggestions = suggestions;

        if let Some(existing) = guard.as_mut() {
            if existing.matches_base(state) {
                if existing.suggestions != suggestions {
                    existing.suggestions = suggestions.clone();
                    existing.index = 0;
                }
                return vec![existing.current().to_string()];
            }

            if existing.matches_current(state) {
                if state.prefix == existing.current() && existing.suggestions != suggestions {
                    suggestions = existing.suggestions.clone();
                } else if existing.suggestions != suggestions {
                    existing.suggestions = suggestions.clone();
                    existing.index = 0;
                }

                if !existing.suggestions.is_empty() {
                    existing.index = (existing.index + 1) % existing.suggestions.len();
                    return vec![existing.current().to_string()];
                }
            }
        }

        let mut cycle = CycleState::new(state, suggestions);
        if cycle.suggestions.len() > 1 && state.prefix == cycle.current() {
            cycle.index = (cycle.index + 1) % cycle.suggestions.len();
        }
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
    } else {
        tokens.pop().unwrap_or_default()
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
#[path = "../../tests/unit/cli/completion_tests.rs"]
mod tests;
