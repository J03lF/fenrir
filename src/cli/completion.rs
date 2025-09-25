use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper, Result as RustylineResult};

/// Simple completer that offers prefix-based suggestions for command tokens.
pub struct SimpleCompleter {
    commands: Vec<String>,
}

impl SimpleCompleter {
    pub fn new(commands: Vec<String>) -> Self {
        Self { commands }
    }

    pub fn commands(&self) -> &[String] {
        &self.commands
    }
}

impl Helper for SimpleCompleter {}

impl Completer for SimpleCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> RustylineResult<(usize, Vec<Pair>)> {
        let (start, matches) = completion_matches(&self.commands, line, pos);
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

impl Hinter for SimpleCompleter {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        None
    }
}

impl Highlighter for SimpleCompleter {}
impl Validator for SimpleCompleter {}

/// Compute matches for the current token in `line` at position `pos`.
pub fn completion_matches(commands: &[String], line: &str, pos: usize) -> (usize, Vec<String>) {
    let pos = pos.min(line.len());
    let start = line[..pos]
        .rfind(|c: char| c.is_whitespace())
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let prefix = &line[start..pos];
    let matches = commands
        .iter()
        .filter(|cmd| cmd.starts_with(prefix))
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
