use crate::cli::commands::registry::CliDependencies;
use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result as RustylineResult};

/// Context-aware completer offering command + entity suggestions.
pub struct ContextualCompleter {
    commands: Vec<String>,
    deps: CliDependencies,
}

impl ContextualCompleter {
    pub fn new(commands: Vec<String>, deps: CliDependencies) -> Self {
        Self { commands, deps }
    }

    pub fn update_commands(&mut self, commands: Vec<String>) {
        self.commands = commands;
    }

    fn command_matches(&self, prefix: &str) -> Vec<String> {
        self.commands
            .iter()
            .filter(|cmd| cmd.starts_with(prefix))
            .cloned()
            .collect()
    }

    fn service_actions() -> &'static [&'static str] {
        &["list", "jobs", "start", "stop", "restart", "status"]
    }

    fn service_ids(&self) -> Vec<String> {
        self.deps
            .services
            .registry()
            .snapshot()
            .into_iter()
            .map(|svc| svc.descriptor.id.to_string())
            .collect()
    }

    fn module_actions() -> &'static [&'static str] {
        &["list", "info", "install", "update", "remove", "publish"]
    }

    fn resource_keywords() -> &'static [&'static str] {
        &["service", "services", "module", "modules"]
    }

    fn suggest(&self, command: Option<&str>, completed_args: &[&str], prefix: &str) -> Vec<String> {
        match command {
            None => self.command_matches(prefix),
            Some("services") | Some("service") | Some("svc") => {
                let arg_pos = completed_args.len();
                match arg_pos {
                    0 => Self::filter(Self::service_actions().iter().copied(), prefix),
                    _ => match completed_args.get(0).copied() {
                        Some("start" | "stop" | "restart" | "status") => self
                            .service_ids()
                            .into_iter()
                            .filter(|id| id.starts_with(prefix))
                            .collect(),
                        Some("jobs") => Vec::new(),
                        _ => Vec::new(),
                    },
                }
            }
            Some("start") | Some("stop") | Some("restart") => {
                let arg_pos = completed_args.len();
                match arg_pos {
                    0 => Self::filter(Self::resource_keywords().iter().copied(), prefix),
                    1 => match completed_args[0] {
                        "service" | "services" => self
                            .service_ids()
                            .into_iter()
                            .filter(|id| id.starts_with(prefix))
                            .collect(),
                        "module" | "modules" => Vec::new(),
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                }
            }
            Some("list") => {
                if completed_args.is_empty() {
                    Self::filter(["services", "jobs", "modules"].iter().copied(), prefix)
                } else {
                    Vec::new()
                }
            }
            Some("modules") | Some("module") => {
                if completed_args.is_empty() {
                    Self::filter(Self::module_actions().iter().copied(), prefix)
                } else {
                    Vec::new()
                }
            }
            _ => self.command_matches(prefix),
        }
    }

    fn filter<'a, I>(iter: I, prefix: &str) -> Vec<String>
    where
        I: IntoIterator<Item = &'a str>,
    {
        iter.into_iter()
            .filter(|item| prefix.is_empty() || item.starts_with(prefix))
            .map(|item| item.to_string())
            .collect()
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
        let pos = pos.min(line.len());
        let head = &line[..pos];
        let has_trailing = head
            .chars()
            .rev()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false);
        let token_start = head
            .rfind(|c: char| c.is_whitespace())
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let prefix = &line[token_start..pos];

        let mut context_tokens: Vec<&str> = head.split_whitespace().collect();
        let mut current_prefix = prefix;
        if !has_trailing && !prefix.is_empty() && !context_tokens.is_empty() {
            context_tokens.pop();
        }
        if has_trailing {
            current_prefix = "";
        }

        let command = context_tokens.first().copied();
        let completed_args: Vec<&str> = if context_tokens.len() > 1 {
            context_tokens[1..].to_vec()
        } else {
            Vec::new()
        };

        let matches = self.suggest(command, &completed_args, current_prefix);

        let pairs = matches
            .into_iter()
            .map(|m| Pair {
                display: m.clone(),
                replacement: m,
            })
            .collect();
        Ok((token_start, pairs))
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
