use anyhow::{anyhow, Context, Result};
use futures::executor::block_on;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use thrussh::server::{Auth, Handle as SessionHandle, Server, Session};
use thrussh::{server, ChannelId, CryptoVec};
use tokio::runtime::Handle as TokioHandle;

use crate::cli::commands::builtins;
use crate::cli::commands::builtins::db_shell::{self, RuntimeExecutor};
use crate::cli::commands::registry::{
    CliDependencies, CommandOutcome, CommandOutput, CommandRegistry, CommandStatus,
    ShellEnvironment,
};
use crate::cli::completion::{self, ContextualCompleter};
use crate::config::AppConfig;
use crate::prompts::{self, PromptContext};
use crate::services::db_shell::DbShellSession;
use crate::services::{AppServices, ServiceStatus};

const HISTORY_MAX: usize = 200;
const COMPLETION_DISPLAY_WIDTH: usize = 80;
const COMPLETION_PADDING: usize = 2;
const COMPLETION_MAX_VISIBLE: usize = 24;

#[derive(Clone)]
struct Handler {
    username: String,
    password_env: String,
    buffer: String,
    main_prompt: String,
    db_prompt: String,
    skip_next_lf: bool,
    escape_state: EscapeState,
    config: Arc<AppConfig>,
    services: Arc<AppServices>,
    dependencies: CliDependencies,
    registry: CommandRegistry,
    completer: Arc<ContextualCompleter>,
    history: Vec<String>,
    history_index: Option<usize>,
    mode: ShellMode,
    db_session: Option<DbShellSession>,
    db_executor: Option<Arc<RuntimeExecutor>>,
    cursor: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EscapeState {
    #[default]
    None,
    Esc,
    Csi,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellMode {
    Main,
    DbShell,
}

impl server::Handler for Handler {
    type Error = anyhow::Error;
    type FutureAuth = futures::future::Ready<Result<(Self, Auth), Self::Error>>;
    type FutureUnit = futures::future::Ready<Result<(Self, Session), Self::Error>>;
    type FutureBool = futures::future::Ready<Result<(Self, Session, bool), Self::Error>>;

    fn finished_auth(self, auth: Auth) -> Self::FutureAuth {
        futures::future::ready(Ok((self, auth)))
    }
    fn finished(self, session: Session) -> Self::FutureUnit {
        futures::future::ready(Ok((self, session)))
    }
    fn finished_bool(self, b: bool, session: Session) -> Self::FutureBool {
        futures::future::ready(Ok((self, session, b)))
    }

    fn auth_password(self, user: &str, password: &str) -> Self::FutureAuth {
        let env_pw = std::env::var(&self.password_env).unwrap_or_default();
        if user == self.username && !env_pw.is_empty() && password == env_pw {
            self.finished_auth(Auth::Accept)
        } else {
            self.finished_auth(Auth::Reject)
        }
    }

    fn channel_open_session(self, channel: ChannelId, mut session: Session) -> Self::FutureUnit {
        {
            let mut writer = SessionWriter::new(&mut session, channel);
            let _ = write!(&mut writer, "{}", prompts::clear_screen_sequence());
            let _ = writeln!(&mut writer, "{}", prompts::banner());
            let _ = writeln!(
                &mut writer,
                "{}",
                prompts::welcome_line(self.config.as_ref())
            );
        }
        Handler::send_prompt(&mut session, channel, self.current_prompt());
        self.finished(session)
    }

    fn data(mut self, channel: ChannelId, data: &[u8], mut session: Session) -> Self::FutureUnit {
        let chunk = String::from_utf8_lossy(data);
        for ch in chunk.chars() {
            if self.skip_next_lf {
                if ch == '\n' {
                    self.skip_next_lf = false;
                    continue;
                }
                self.skip_next_lf = false;
            }

            match self.escape_state {
                EscapeState::Esc => {
                    if ch == '[' {
                        self.escape_state = EscapeState::Csi;
                    } else {
                        self.escape_state = EscapeState::None;
                    }
                    continue;
                }
                EscapeState::Csi => {
                    if ch == 'A' {
                        self.history_prev(&mut session, channel);
                    } else if ch == 'B' {
                        self.history_next(&mut session, channel);
                    } else if ch == 'C' {
                        self.move_cursor_right(&mut session, channel);
                    } else if ch == 'D' {
                        self.move_cursor_left(&mut session, channel);
                    }
                    if ('@'..='~').contains(&ch) {
                        self.escape_state = EscapeState::None;
                    }
                    continue;
                }
                EscapeState::None => {}
            }

            match ch {
                '\u{1b}' => {
                    self.escape_state = EscapeState::Esc;
                    continue;
                }
                '\r' => {
                    self.skip_next_lf = true;
                    session.data(channel, CryptoVec::from_slice(b"\r\n"));
                    if !self.process_buffer(channel, &mut session) {
                        return self.finished(session);
                    }
                }
                '\n' => {
                    session.data(channel, CryptoVec::from_slice(b"\r\n"));
                    if !self.process_buffer(channel, &mut session) {
                        return self.finished(session);
                    }
                }
                '\t' => {
                    self.handle_tab(channel, &mut session);
                }
                '\u{8}' | '\u{7f}' => {
                    if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
                        let start = self.cursor - prev.len_utf8();
                        self.buffer.drain(start..self.cursor);
                        self.cursor = start;
                        self.render_buffer(&mut session, channel);
                    } else {
                        session.data(channel, CryptoVec::from_slice(b"\x07"));
                    }
                }
                '\u{3}' => {
                    self.buffer.clear();
                    self.cursor = 0;
                    session.data(channel, CryptoVec::from_slice(b"^C\r\n"));
                    self.history_index = None;
                    Handler::send_prompt(&mut session, channel, self.current_prompt());
                }
                ch => {
                    if ch.is_control() {
                        continue;
                    }
                    let mut buf = [0u8; 4];
                    let encoded = ch.encode_utf8(&mut buf);
                    let insert_at = self.cursor;
                    self.buffer.insert_str(insert_at, encoded);
                    self.cursor += encoded.len();
                    self.history_index = None;
                    self.render_buffer(&mut session, channel);
                }
            }
        }
        self.finished(session)
    }
}

impl Handler {
    fn process_buffer(&mut self, channel: ChannelId, session: &mut Session) -> bool {
        let cmd = self.buffer.trim().to_string();
        self.buffer.clear();
        self.cursor = 0;
        self.history_index = None;

        if cmd.is_empty() {
            Handler::send_prompt(session, channel, self.current_prompt());
            return true;
        }

        self.record_history(&cmd);
        if matches!(self.mode, ShellMode::DbShell) {
            return self.process_db_command(cmd, channel, session);
        }
        let mut parts = cmd.split_whitespace();
        if let Some(name) = parts.next() {
            let args: Vec<&str> = parts.collect();
            let output = Arc::new(SshCommandOutput::new(session.handle(), channel));
            let deps = self.dependencies.with_output(output);
            let mut writer = SessionWriter::new(session, channel);
            match self
                .registry
                .execute(name, &args, &deps, &mut writer, ShellEnvironment::Ssh)
            {
                Ok(CommandStatus::Executed(CommandOutcome::Continue)) => {
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
                Ok(CommandStatus::Executed(CommandOutcome::ExitShell)) => {
                    session.close(channel);
                    false
                }
                Ok(CommandStatus::Executed(CommandOutcome::EnterDbShell)) => {
                    if !self.services.db_shell.is_enabled() {
                        let _ = writeln!(
                            &mut writer,
                            "DB-Shell ist deaktiviert. Nutze 'start service db-shell'."
                        );
                        Handler::send_prompt(session, channel, self.current_prompt());
                        return true;
                    }
                    self.mode = ShellMode::DbShell;
                    self.buffer.clear();
                    self.history_index = None;
                    self.cursor = 0;
                    let db_session = self.services.db_shell.create_session();
                    let current_engine = db_session.current_engine();
                    let engines = db_session
                        .available_engines()
                        .iter()
                        .map(|engine| engine.to_string())
                        .collect::<Vec<_>>();
                    match RuntimeExecutor::new() {
                        Ok(executor) => {
                            self.db_executor = Some(Arc::new(executor));
                            self.db_session = Some(db_session);
                            let _ = writeln!(
                                &mut writer,
                                "DB-Shell aktiv. Aktueller Engine: {current_engine}"
                            );
                            if !engines.is_empty() {
                                let _ = writeln!(
                                    &mut writer,
                                    "Verfügbare Engines: {}",
                                    engines.join(", ")
                                );
                            }
                            let _ = writeln!(
                                &mut writer,
                                "Hinweis: Destruktive Befehle benötigen '--force'."
                            );
                            Handler::send_prompt(session, channel, self.current_prompt());
                            true
                        }
                        Err(err) => {
                            tracing::error!(error = %err, "failed to create runtime for db-shell over ssh");
                            self.mode = ShellMode::Main;
                            let _ = writeln!(
                                &mut writer,
                                "DB-Shell konnte nicht gestartet werden ({err})"
                            );
                            Handler::send_prompt(session, channel, self.current_prompt());
                            true
                        }
                    }
                }
                Ok(CommandStatus::NotFound) => {
                    let _ = writeln!(&mut writer, "unbekannter Befehl: {}", name);
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
                Err(err) => {
                    tracing::error!(command = %cmd, error = %err, "ssh command execution failed");
                    let _ = writeln!(&mut writer, "Fehler bei der Befehlsausführung");
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
            }
        } else {
            Handler::send_prompt(session, channel, self.current_prompt());
            true
        }
    }

    fn send_prompt(session: &mut Session, channel: ChannelId, prompt: &str) {
        session.data(channel, CryptoVec::from_slice(prompt.as_bytes()));
    }

    fn process_db_command(
        &mut self,
        command: String,
        channel: ChannelId,
        session: &mut Session,
    ) -> bool {
        let mut writer = SessionWriter::new(session, channel);
        let trimmed = command.trim();
        if trimmed.is_empty() {
            Handler::send_prompt(session, channel, self.current_prompt());
            return true;
        }

        if self.db_session.is_none() {
            if !self.services.db_shell.is_enabled() {
                let _ = writeln!(
                    &mut writer,
                    "DB-Shell ist deaktiviert. Zurück zur Hauptshell."
                );
                self.mode = ShellMode::Main;
                Handler::send_prompt(session, channel, self.current_prompt());
                return true;
            }
            self.db_session = Some(self.services.db_shell.create_session());
        }

        let executor = match self.db_executor.as_ref() {
            Some(exec) => Arc::clone(exec),
            None => match RuntimeExecutor::new() {
                Ok(exec) => {
                    let arc = Arc::new(exec);
                    self.db_executor = Some(Arc::clone(&arc));
                    arc
                }
                Err(err) => {
                    tracing::error!(error = %err, "failed to initialize runtime for db-shell command");
                    let _ = writeln!(&mut writer, "DB-Shell Fehler: {err}");
                    self.services.registry().set_status(
                        "db-shell",
                        ServiceStatus::Degraded,
                        Some(format!("Runtime Fehler: {err}")),
                    );
                    Handler::send_prompt(session, channel, self.current_prompt());
                    return true;
                }
            },
        };

        if let Some(db_session) = self.db_session.as_mut() {
            if !self.services.db_shell.is_enabled() {
                let _ = writeln!(
                    &mut writer,
                    "DB-Shell wurde deaktiviert. Rückkehr zur Hauptshell."
                );
                self.mode = ShellMode::Main;
                self.db_session = None;
                self.db_executor = None;
                Handler::send_prompt(session, channel, self.current_prompt());
                return true;
            }
            match db_shell::apply_command(db_session, trimmed, executor.as_ref(), &mut writer) {
                Ok(true) => {
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
                Ok(false) => {
                    self.mode = ShellMode::Main;
                    self.history_index = None;
                    self.db_session = None;
                    self.db_executor = None;
                    self.services.registry().set_status(
                        "db-shell",
                        ServiceStatus::Active,
                        Some("Bereit für neue Sessions".to_string()),
                    );
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
                Err(err) => {
                    tracing::error!(error = %err, "db-shell command failed over ssh");
                    let _ = writeln!(&mut writer, "DB-Shell Fehler: {err}");
                    self.services.registry().set_status(
                        "db-shell",
                        ServiceStatus::Degraded,
                        Some(format!("Fehler: {err}")),
                    );
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
            }
        } else {
            let _ = writeln!(&mut writer, "DB-Shell nicht initialisiert");
            Handler::send_prompt(session, channel, self.current_prompt());
            true
        }
    }

    fn handle_tab(&mut self, channel: ChannelId, session: &mut Session) {
        match self.mode {
            ShellMode::Main => self.complete_main(channel, session),
            ShellMode::DbShell => self.complete_db(channel, session),
        }
    }

    fn complete_main(&mut self, channel: ChannelId, session: &mut Session) {
        let (start, suggestions) = self.completer.suggestions_for(&self.buffer, self.cursor);
        if suggestions.is_empty() {
            session.data(channel, CryptoVec::from_slice(b"\x07"));
            return;
        }

        let before = self.buffer[..start].to_string();
        let remainder = self.buffer[self.cursor..].to_string();
        let prefix = self.buffer[start..self.cursor].to_string();
        let mut updated_buffer = false;
        let mut suggestions_shown = false;

        let command_completion = start == 0
            && self.cursor == prefix.len()
            && !prefix.is_empty()
            && self.registry.get(prefix.as_str()).is_some();
        let should_append_space = command_completion && remainder.is_empty();

        if suggestions.len() > 1 && !suggestions_shown {
            if let Some(first) = suggestions.first() {
                if prefix.as_str() != first.as_str() {
                    self.show_suggestions(&suggestions, channel, session);
                    suggestions_shown = true;
                }
            }
        }

        if suggestions.len() == 1 {
            let completion = &suggestions[0];
            if completion == prefix.as_str() && remainder.is_empty() {
                let (_, cycle_matches) =
                    self.completer.cycle_suggestions(&self.buffer, self.cursor);
                if let Some(next) = cycle_matches.first() {
                    if next != completion {
                        self.buffer = before.clone();
                        self.buffer.push_str(next);
                        self.cursor = self.buffer.len();
                        self.buffer.push_str(&remainder);
                        self.render_buffer(session, channel);
                        return;
                    } else if !should_append_space {
                        session.data(channel, CryptoVec::from_slice(b"\x07"));
                        return;
                    }
                } else if !should_append_space {
                    session.data(channel, CryptoVec::from_slice(b"\x07"));
                    return;
                }
            }
            self.buffer = before.clone();
            self.buffer.push_str(completion);
            self.cursor = self.buffer.len();
            if should_append_space {
                self.buffer.push(' ');
                self.cursor += 1;
            }
            self.buffer.push_str(&remainder);
            self.render_buffer(session, channel);
            return;
        }

        if let Some(common) = completion::longest_common_prefix(&suggestions) {
            if common.len() > prefix.len() {
                self.buffer = before.clone();
                self.buffer.push_str(&common);
                self.cursor = self.buffer.len();
                self.buffer.push_str(&remainder);
                updated_buffer = true;
            }
        }

        if !updated_buffer {
            let (_, cycle_matches) = self.completer.cycle_suggestions(&self.buffer, self.cursor);
            if let Some(completion) = cycle_matches.first() {
                if completion != prefix.as_str() {
                    self.buffer = before.clone();
                    self.buffer.push_str(completion);
                    self.cursor = self.buffer.len();
                    self.buffer.push_str(&remainder);
                    updated_buffer = true;
                }
            }
        }

        if suggestions.len() > 1 && !updated_buffer && !suggestions_shown {
            self.show_suggestions(&suggestions, channel, session);
        }

        if updated_buffer {
            self.render_buffer(session, channel);
            return;
        }

        self.render_buffer(session, channel);
    }

    fn show_suggestions(&self, suggestions: &[String], channel: ChannelId, session: &mut Session) {
        session.data(channel, CryptoVec::from_slice(b"\r\n"));
        self.print_suggestions_compact(suggestions, channel, session);
    }

    fn print_suggestions_compact(
        &self,
        suggestions: &[String],
        channel: ChannelId,
        session: &mut Session,
    ) {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut entries: Vec<&str> = Vec::new();
        for entry in suggestions {
            let entry = entry.as_str();
            if seen.insert(entry) {
                entries.push(entry);
            }
        }

        if entries.is_empty() {
            return;
        }

        let total = entries.len();
        let hidden = total.saturating_sub(COMPLETION_MAX_VISIBLE);
        if hidden > 0 {
            entries.truncate(COMPLETION_MAX_VISIBLE);
        }

        let max_len = entries.iter().map(|entry| entry.len()).max().unwrap_or(0);
        let mut column_width = max_len.saturating_add(COMPLETION_PADDING);
        if column_width == 0 {
            column_width = COMPLETION_PADDING;
        }
        column_width = column_width.min(COMPLETION_DISPLAY_WIDTH);
        let mut columns = COMPLETION_DISPLAY_WIDTH / column_width;
        if columns == 0 {
            columns = 1;
        }
        columns = columns.min(entries.len());

        let mut writer = SessionWriter::new(session, channel);
        for chunk in entries.chunks(columns) {
            for (index, entry) in chunk.iter().enumerate() {
                if columns == 1 || index + 1 == chunk.len() {
                    let _ = write!(&mut writer, "{entry}");
                } else {
                    let _ = write!(
                        &mut writer,
                        "{entry:<width$}",
                        entry = entry,
                        width = column_width,
                    );
                }
            }
            let _ = writeln!(&mut writer);
        }

        if hidden > 0 {
            let suffix = if hidden == 1 { "" } else { "s" };
            let _ = writeln!(
                &mut writer,
                "... {hidden} more suggestion{suffix} hidden (press TAB to cycle)."
            );
        }
    }

    fn complete_db(&mut self, channel: ChannelId, session: &mut Session) {
        let commands = db_shell::completion_words(self.db_session.as_ref());
        if commands.is_empty() {
            session.data(channel, CryptoVec::from_slice(b"\x07"));
            return;
        }
        let (start, matches) = completion::completion_matches(&commands, &self.buffer, self.cursor);
        if matches.is_empty() {
            session.data(channel, CryptoVec::from_slice(b"\x07"));
            return;
        }

        let before = self.buffer[..start].to_string();
        let remainder = self.buffer[self.cursor..].to_string();
        let prefix_len = self.cursor - start;

        if matches.len() == 1 {
            let completion = &matches[0];
            self.buffer = before.clone();
            self.buffer.push_str(completion);
            self.cursor = self.buffer.len();
            if remainder.is_empty() {
                self.buffer.push(' ');
                self.cursor += 1;
            }
            self.buffer.push_str(&remainder);
            self.render_buffer(session, channel);
            return;
        }

        if let Some(common) = completion::longest_common_prefix(&matches) {
            if common.len() > prefix_len {
                self.buffer = before.clone();
                self.buffer.push_str(&common);
                self.cursor = self.buffer.len();
                self.buffer.push_str(&remainder);
                self.render_buffer(session, channel);
                return;
            }
        }

        session.data(channel, CryptoVec::from_slice(b"\r\n"));
        {
            let mut writer = SessionWriter::new(session, channel);
            for entry in &matches {
                let _ = writeln!(&mut writer, "{entry}");
            }
        }
        self.render_buffer(session, channel);
    }

    fn current_prompt(&self) -> &str {
        match self.mode {
            ShellMode::Main => &self.main_prompt,
            ShellMode::DbShell => &self.db_prompt,
        }
    }

    fn history_prev(&mut self, session: &mut Session, channel: ChannelId) {
        if self.history.is_empty() {
            return;
        }
        let next_index = match self.history_index {
            Some(0) => 0,
            Some(idx) => idx.saturating_sub(1),
            None => self.history.len() - 1,
        };
        self.history_index = Some(next_index);
        self.buffer.clear();
        self.buffer.push_str(&self.history[next_index]);
        self.cursor = self.buffer.len();
        self.render_buffer(session, channel);
    }

    fn history_next(&mut self, session: &mut Session, channel: ChannelId) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            Some(idx) if idx + 1 < self.history.len() => {
                let next_index = idx + 1;
                self.history_index = Some(next_index);
                self.buffer.clear();
                self.buffer.push_str(&self.history[next_index]);
            }
            Some(_) => {
                self.history_index = None;
                self.buffer.clear();
            }
            None => return,
        }
        self.cursor = self.buffer.len();
        self.render_buffer(session, channel);
    }

    fn render_buffer(&mut self, session: &mut Session, channel: ChannelId) {
        if self.cursor > self.buffer.len() {
            self.cursor = self.buffer.len();
        }
        session.data(channel, CryptoVec::from_slice(b"\r"));
        Handler::send_prompt(session, channel, self.current_prompt());
        session.data(channel, CryptoVec::from_slice(b"\x1b[K"));
        if !self.buffer.is_empty() {
            session.data(channel, CryptoVec::from_slice(self.buffer.as_bytes()));
        }
        let tail_len = self.buffer[self.cursor..].chars().count();
        if tail_len > 0 {
            let seq = format!("\x1b[{}D", tail_len);
            session.data(channel, CryptoVec::from_slice(seq.as_bytes()));
        }
    }

    fn move_cursor_left(&mut self, session: &mut Session, channel: ChannelId) {
        if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
            self.cursor = self.cursor.saturating_sub(prev.len_utf8());
            session.data(channel, CryptoVec::from_slice(b"\x1b[D"));
        }
    }

    fn move_cursor_right(&mut self, session: &mut Session, channel: ChannelId) {
        if let Some(next) = self.buffer[self.cursor..].chars().next() {
            self.cursor = (self.cursor + next.len_utf8()).min(self.buffer.len());
            session.data(channel, CryptoVec::from_slice(b"\x1b[C"));
        }
    }

    fn record_history(&mut self, command: &str) {
        if command.is_empty() {
            return;
        }
        if self.history.last().map_or(false, |last| last == command) {
            return;
        }
        if self.history.len() >= HISTORY_MAX {
            self.history.remove(0);
        }
        self.history.push(command.to_string());
    }
}

pub async fn start(cfg: &Arc<AppConfig>, services: &Arc<AppServices>) -> Result<()> {
    let config = server::Config {
        auth_rejection_time: Duration::from_secs(1),
        ..Default::default()
    };
    let mut config = config;
    let host_key = load_or_create_host_key(&cfg.server.ssh.host_key_path)
        .with_context(|| format!("loading ssh host key from {}", cfg.server.ssh.host_key_path))?;
    config.keys.push(host_key);
    let config = Arc::new(config);

    struct Factory {
        username: String,
        password_env: String,
        config: Arc<AppConfig>,
        services: Arc<AppServices>,
    }
    impl Server for Factory {
        type Handler = Handler;
        fn new(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
            let prompt_ctx = PromptContext {
                user: self.username.clone(),
                host: self.config.server.ssh.server_name.clone(),
                role: "ssh".to_string(),
                transport: "ssh".to_string(),
            };
            let prompts = prompts::prompt_set(self.config.as_ref(), &prompt_ctx);
            let services = Arc::clone(&self.services);
            let config = Arc::clone(&self.config);
            let dependencies =
                CliDependencies::new(Arc::clone(&self.config), Arc::clone(&self.services));
            let registry = builtins::build_registry();
            let mut completer = ContextualCompleter::new(
                registry.shapes(),
                dependencies.clone(),
                ShellEnvironment::Ssh,
            );
            completer.update_catalog(registry.shapes());
            Handler {
                username: self.username.clone(),
                password_env: self.password_env.clone(),
                buffer: String::new(),
                main_prompt: prompts.main_transport,
                db_prompt: prompts.db_transport,
                skip_next_lf: false,
                escape_state: EscapeState::None,
                config,
                services,
                dependencies,
                registry,
                completer: Arc::new(completer),
                history: Vec::new(),
                history_index: None,
                mode: ShellMode::Main,
                db_session: None,
                db_executor: None,
                cursor: 0,
            }
        }
    }

    tracing::info!(host = %cfg.server.ssh.host, port = cfg.server.ssh.port, "starting SSH server");
    let bind_addr = format!("{}:{}", cfg.server.ssh.host, cfg.server.ssh.port);
    services.registry().set_status(
        "ssh-server",
        ServiceStatus::Active,
        Some(format!("Lauscht auf {bind_addr}")),
    );

    let server = Factory {
        username: cfg.server.ssh.user.clone(),
        password_env: "FENRIR_SSH_PASSWORD".to_string(),
        config: Arc::clone(cfg),
        services: Arc::clone(services),
    };
    match thrussh::server::run(config, &bind_addr, server).await {
        Ok(()) => {
            services.registry().set_status(
                "ssh-server",
                ServiceStatus::Stopped,
                Some("Listener beendet".to_string()),
            );
            Ok(())
        }
        Err(err) => {
            services.registry().set_status(
                "ssh-server",
                ServiceStatus::Failed,
                Some(format!("Fehler: {err}")),
            );
            Err(err.into())
        }
    }
}

fn load_or_create_host_key(path: &str) -> Result<thrussh_keys::key::KeyPair> {
    let key_path = Path::new(path);
    if key_path.exists() {
        if let Ok(key) = thrussh_keys::load_secret_key(key_path, None) {
            return Ok(key);
        }
        let bytes = fs::read(key_path)?;
        let mut secret = thrussh_keys::key::ed25519::SecretKey::new_zeroed();
        if bytes.len() != secret.key.len() {
            return Err(anyhow!(
                "unsupported host key format in {}",
                key_path.display()
            ));
        }
        secret.key.clone_from_slice(&bytes);
        return Ok(thrussh_keys::key::KeyPair::Ed25519(secret));
    }

    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let key = thrussh_keys::key::KeyPair::generate_ed25519()
        .ok_or_else(|| anyhow!("failed to generate ed25519 host key"))?;
    if let thrussh_keys::key::KeyPair::Ed25519(secret) = &key {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(key_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        file.write_all(&secret.key)?;
        tracing::info!(path = %key_path.display(), "generated new SSH host key");
    }
    Ok(key)
}

struct SessionWriter<'a> {
    session: &'a mut Session,
    channel: ChannelId,
}

impl<'a> SessionWriter<'a> {
    fn new(session: &'a mut Session, channel: ChannelId) -> Self {
        Self { session, channel }
    }
}

impl Write for SessionWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut converted: Vec<u8> = Vec::with_capacity(buf.len() * 2);
        for &byte in buf {
            if byte == b'\n' {
                converted.push(b'\r');
                converted.push(b'\n');
            } else {
                converted.push(byte);
            }
        }
        self.session
            .data(self.channel, CryptoVec::from_slice(&converted));
        self.session
            .flush()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.session
            .flush()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))
    }
}

#[derive(Clone)]
struct SshCommandOutput {
    handle: SessionHandle,
    channel: ChannelId,
}

impl SshCommandOutput {
    fn new(handle: SessionHandle, channel: ChannelId) -> Self {
        Self { handle, channel }
    }
}

impl CommandOutput for SshCommandOutput {
    fn push(&self, text: &str) {
        let channel = self.channel;
        let mut converted = Vec::with_capacity(text.len() * 2);
        for &byte in text.as_bytes() {
            if byte == b'\n' {
                converted.push(b'\r');
                converted.push(b'\n');
            } else {
                converted.push(byte);
            }
        }
        let mut handle = self.handle.clone();
        if let Ok(runtime) = TokioHandle::try_current() {
            runtime.spawn(async move {
                let mut payload = CryptoVec::new();
                payload.extend(&converted);
                let _ = handle.data(channel, payload).await;
            });
        } else {
            let mut payload = CryptoVec::new();
            payload.extend(&converted);
            let _ = block_on(handle.data(channel, payload));
        }
    }
}
