use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use russh::cipher;
use russh::server::{Auth, Handle as SessionHandle, Msg, Server, Session};
use russh::{server, Channel, ChannelId, CryptoVec, Preferred};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::runtime::Handle as TokioHandle;
use tokio::sync::mpsc;

use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::cli::commands::builtins;
use crate::cli::commands::builtins::db_shell::{
    self, build_completion_list_with_tables, DbCompletionCatalog, RuntimeExecutor,
};
use crate::cli::commands::registry::{
    parse_confirmation_answer, CliDependencies, CommandOutcome, CommandOutput, CommandRegistry,
    CommandStatus, ConfirmationRequest, ShellEnvironment,
};
use crate::cli::completion::{self, ContextualCompleter};
use crate::config::{AppConfig, IdentityProviderKind};
use crate::infra::telemetry;
use crate::prompts::{self, PromptContext};
use crate::security::auth::Role;
use crate::security::identity::{IdentityError, IdentityUserProfile};
use crate::services::db_shell::{DbShellSession, DESTRUCTIVE_FORCE_WARNING};
use crate::services::{AppServices, ServiceDiagnostics, ServiceStatus};
use crate::utils::messages::cli::shell::{
    outcome as cli_shell_outcome, runner as cli_shell_runner,
};
use crate::utils::messages::infra::ssh as ssh_messages;
use semver::Version;
use tracing::warn;

use super::session_writer::SessionWriter;

const HISTORY_MAX: usize = 200;
const COMPLETION_DISPLAY_WIDTH: usize = 80;
const COMPLETION_PADDING: usize = 2;
const COMPLETION_MAX_VISIBLE: usize = 24;
const DB_CONTINUATION_PROMPT: &str = "...> ";
const SSH_TRANSPORT: &str = "ssh";
const SSH_DEFAULT_ROLE: &str = "ssh";
const SSH_PASSWORD_ENV: &str = "FENRIR_SSH_PASSWORD";

/// Calculate the visible character width of a string, stripping ANSI escape sequences.
fn visible_width(s: &str) -> usize {
    let mut width = 0;
    let mut in_escape = false;
    for ch in s.chars() {
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
            continue;
        }
        if ch == '\x1b' {
            in_escape = true;
            continue;
        }
        if !ch.is_control() {
            width += 1;
        }
    }
    width
}

struct Handler {
    username: String,
    password_env: Option<String>,
    identity_required: bool,
    identity_profile: Option<IdentityUserProfile>,
    role_label: String,
    buffer: String,
    main_prompt: String,
    db_prompt: String,
    db_continuation_prompt: String,
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
    db_completion_words: Vec<String>,
    db_completion_tables: HashSet<String>,
    db_multiline_buffer: String,
    cursor: usize,
    rendered_line_count: usize,
    rendered_cursor_row: usize,
    pending_confirmation: Option<ConfirmationRequest>,
    channel: Option<Channel<Msg>>,
}

struct ServiceProbe {
    diagnostics: Arc<ServiceDiagnostics>,
    service_id: &'static str,
    started_at: Instant,
    success: bool,
}

impl ServiceProbe {
    fn new(services: &Arc<AppServices>, service_id: &'static str) -> Self {
        Self {
            diagnostics: services.diagnostics(),
            service_id,
            started_at: Instant::now(),
            success: false,
        }
    }

    fn mark_success(&mut self) {
        self.success = true;
    }
}

impl Drop for ServiceProbe {
    fn drop(&mut self) {
        let latency_ms = self.started_at.elapsed().as_secs_f64() * 1000.0;
        self.diagnostics
            .record_probe(self.service_id, latency_ms, self.success);
    }
}

impl Handler {
    fn password_env_accepts(&self, user: &str, password: &str) -> bool {
        if user != self.username {
            return false;
        }
        let env_pw = self
            .password_env
            .as_ref()
            .and_then(|key| std::env::var(key).ok())
            .unwrap_or_default();
        !env_pw.is_empty() && password == env_pw
    }

    fn record_login_audit(&self, user: &str, success: bool, reason: Option<&str>) {
        let role = self
            .identity_profile
            .as_ref()
            .map(|p| p.role.as_str())
            .unwrap_or("ssh");

        let outcome = if success {
            AuditOutcome::Success
        } else {
            AuditOutcome::Denied
        };

        let mut metadata = AuditMetadata::default().insert("transport", "ssh");
        if let Some(r) = reason {
            metadata = metadata.insert("reason", r);
        }

        if let Ok(event) = AuditEvent::builder()
            .actor(AuditActor::User {
                user_id: user.to_string(),
                role: role.to_string(),
            })
            .action("ssh.login")
            .target(format!("ssh://{}", user))
            .outcome(outcome)
            .metadata(metadata)
            .build()
        {
            let _ = self.services.record_audit(event);
        }
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        // Decrement active SSH connection count
        telemetry::decrement_counter("ssh.connections.active", 1);
    }
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

#[async_trait]
impl server::Handler for Handler {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if self.identity_required {
            let allowed_user = self.config.server.ssh.user.clone();

            // Only allow the configured user (from config/FENRIR_USER)
            if user != allowed_user {
                warn!(
                    ssh_user = %user,
                    allowed_user = %allowed_user,
                    "login rejected: user not authorized"
                );
                self.record_login_audit(user, false, Some("user not authorized"));
                return Ok(Auth::Reject {
                    proceed_with_methods: None,
                });
            }

            match self.authenticate_identity(user, password) {
                Ok(true) => {
                    // Normal authentication success
                    self.record_login_audit(user, true, None);
                    Ok(Auth::Accept)
                }
                Ok(false) => {
                    // Password not set - entering setup mode
                    // Accept auth to allow channel for setup dialog
                    Ok(Auth::Accept)
                }
                Err(err) => {
                    warn!(ssh_user = %user, error = %err, "identity authentication rejected");
                    self.record_login_audit(user, false, Some(&err.to_string()));
                    Ok(Auth::Reject {
                        proceed_with_methods: None,
                    })
                }
            }
        } else if self.password_env_accepts(user, password) {
            self.record_login_audit(user, true, None);
            Ok(Auth::Accept)
        } else {
            self.record_login_audit(user, false, Some("invalid credentials"));
            Ok(Auth::Reject {
                proceed_with_methods: None,
            })
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        self.channel = Some(channel);
        Ok(true)
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let mut writer = SessionWriter::new(session, channel);
        let prompt_context = self.prompt_context();
        let _ = write!(&mut writer, "{}", prompts::clear_screen_sequence());
        let _ = writeln!(
            &mut writer,
            "{}",
            prompts::banner(self.config.as_ref(), &prompt_context)
        );
        let _ = writeln!(
            &mut writer,
            "{}",
            prompts::welcome_line(self.config.as_ref())
        );
        Handler::send_prompt(session, channel, self.current_prompt());
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let chunk = String::from_utf8_lossy(data);
        for ch in chunk.chars() {
            if self.skip_next_lf {
                if ch == '\n' {
                    self.skip_next_lf = false;
                    continue;
                }
                self.skip_next_lf = false;
            }

            if self.pending_confirmation.is_some() {
                if !self.handle_confirmation_input(ch, channel, session) {
                    return Ok(());
                }
                continue;
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
                        self.history_prev(session, channel);
                    } else if ch == 'B' {
                        self.history_next(session, channel);
                    } else if ch == 'C' {
                        self.move_cursor_right(session, channel);
                    } else if ch == 'D' {
                        self.move_cursor_left(session, channel);
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
                    // Move to the last rendered line before advancing
                    self.move_to_last_rendered_line(session, channel);
                    session.data(channel, CryptoVec::from_slice(b"\r\n"));
                    self.rendered_line_count = 1;
                    self.rendered_cursor_row = 0;
                    if !self.process_buffer(channel, session) {
                        return Ok(());
                    }
                }
                '\n' => {
                    self.move_to_last_rendered_line(session, channel);
                    session.data(channel, CryptoVec::from_slice(b"\r\n"));
                    self.rendered_line_count = 1;
                    self.rendered_cursor_row = 0;
                    if !self.process_buffer(channel, session) {
                        return Ok(());
                    }
                }
                '\t' => {
                    self.handle_tab(channel, session);
                }
                '\u{8}' | '\u{7f}' => {
                    if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
                        let start = self.cursor - prev.len_utf8();
                        self.buffer.drain(start..self.cursor);
                        self.cursor = start;
                        self.render_buffer(session, channel);
                    } else {
                        session.data(channel, CryptoVec::from_slice(b"\x07"));
                    }
                }
                '\u{3}' => {
                    self.move_to_last_rendered_line(session, channel);
                    self.buffer.clear();
                    self.cursor = 0;
                    self.rendered_line_count = 1;
                    self.rendered_cursor_row = 0;
                    session.data(channel, CryptoVec::from_slice(b"^C\r\n"));
                    self.history_index = None;
                    self.db_multiline_buffer.clear();
                    Handler::send_prompt(session, channel, self.current_prompt());
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
                    self.render_buffer(session, channel);
                }
            }
        }
        Ok(())
    }
}

impl Handler {
    fn refresh_prompts(&mut self) {
        let prompt_ctx = self.prompt_context();
        let prompts = prompts::prompt_set(self.config.as_ref(), &prompt_ctx);
        self.main_prompt = prompts.main_transport;
        self.db_prompt = prompts.db_transport;
        self.db_continuation_prompt = DB_CONTINUATION_PROMPT.to_string();
    }

    fn prompt_context(&self) -> PromptContext {
        let host = self
            .config
            .app
            .profile
            .as_ref()
            .filter(|p| !p.is_empty())
            .cloned()
            .unwrap_or_else(|| self.config.server.ssh.server_name.clone());
        PromptContext {
            user: self.username.clone(),
            host,
            role: self.role_label.clone(),
            transport: SSH_TRANSPORT.to_string(),
        }
    }

    fn set_identity_context(&mut self, profile: IdentityUserProfile) {
        let display = profile
            .display_name
            .clone()
            .filter(|label| !label.trim().is_empty())
            .unwrap_or_else(|| profile.user_id.clone());
        self.username = display;
        self.role_label = profile.role.as_str().to_string();
        self.identity_profile = Some(profile.clone());
        let actor = AuditActor::User {
            user_id: profile.user_id.clone(),
            role: profile.role.as_str().to_string(),
        };
        self.dependencies = self.dependencies.with_actor(actor);
        self.refresh_prompts();
    }

    /// Authenticate user via identity provider.
    /// Returns `Ok(true)` for successful auth, `Ok(false)` for password setup needed.
    fn authenticate_identity(&mut self, user: &str, password: &str) -> Result<bool, IdentityError> {
        let identity = self
            .services
            .identity()
            .ok_or_else(|| IdentityError::Invalid("identity provider not available".into()))?;

        let result = if TokioHandle::try_current().is_ok() {
            tokio::task::block_in_place(|| identity.authenticate_user(user, password))
        } else {
            identity.authenticate_user(user, password)
        };

        match result {
            Ok(profile) => {
                if !matches!(profile.role, Role::Admin) {
                    return Err(IdentityError::Unauthorized(
                        "insufficient role for SSH access".into(),
                    ));
                }
                self.set_identity_context(profile);
                Ok(true)
            }
            Err(IdentityError::PasswordNotSet { user_id }) => {
                // Password not set - user must run setup script first
                warn!(user = %user_id, "password not configured - run ./scripts/fenrir-setup.sh first");
                Err(IdentityError::Unauthorized(
                    "password not configured - run ./scripts/fenrir-setup.sh first".into(),
                ))
            }
            Err(err) => Err(err),
        }
    }

    fn process_buffer(&mut self, channel: ChannelId, session: &mut Session) -> bool {
        let raw_line = self.buffer.clone();
        let cmd = raw_line.trim().to_string();
        self.buffer.clear();
        self.cursor = 0;
        self.history_index = None;
        let mut ssh_probe = ServiceProbe::new(&self.services, "ssh-server");

        if matches!(self.mode, ShellMode::DbShell) {
            return self.process_db_line(raw_line, cmd, channel, session, &mut ssh_probe);
        }

        if cmd.is_empty() {
            Handler::send_prompt(session, channel, self.current_prompt());
            ssh_probe.mark_success();
            return true;
        }
        self.record_history(&cmd);
        let mut parts = cmd.split_whitespace();
        if let Some(name) = parts.next() {
            let args: Vec<&str> = parts.collect();
            let output = Arc::new(SshCommandOutput::new(session.handle(), channel));
            let deps = self.dependencies.with_output(output);
            let mut writer = SessionWriter::new(session, channel);
            let result =
                self.registry
                    .execute(name, &args, &deps, &mut writer, ShellEnvironment::Ssh);
            match result {
                Ok(CommandStatus::Executed(outcome)) => {
                    ssh_probe.mark_success();
                    self.handle_command_outcome(outcome, channel, session)
                }
                Ok(CommandStatus::NotFound) => {
                    let suggestions = self.registry.find_similar(name);
                    self.render_command_not_found(&mut writer, name, suggestions);
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
                Err(err) => {
                    tracing::error!(command = %cmd, error = %err, "ssh command execution failed");
                    let _ = writeln!(&mut writer, "{}", ssh_messages::command_execution_failed());
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

    fn render_command_not_found(
        &self,
        writer: &mut SessionWriter,
        command: &str,
        suggestions: Vec<String>,
    ) {
        // Compact inline format
        let _ = writeln!(
            writer,
            "\x1b[38;5;203m✗\x1b[0m Command '\x1b[38;5;79m{}\x1b[0m' not found\r",
            command
        );

        if !suggestions.is_empty() {
            let suggestions_str = suggestions
                .iter()
                .map(|s| format!("\x1b[38;5;79m{}\x1b[0m", s))
                .collect::<Vec<_>>()
                .join("\x1b[38;5;245m,\x1b[0m ");

            let _ = writeln!(
                writer,
                "  \x1b[38;5;245m→\x1b[0m Did you mean: {}\r",
                suggestions_str
            );
        }
    }

    fn process_db_line(
        &mut self,
        raw_line: String,
        trimmed_line: String,
        channel: ChannelId,
        session: &mut Session,
        ssh_probe: &mut ServiceProbe,
    ) -> bool {
        let trimmed = trimmed_line.as_str();
        let has_buffer = !self.db_multiline_buffer.is_empty();

        if trimmed.is_empty() {
            Handler::send_prompt(session, channel, self.current_prompt());
            ssh_probe.mark_success();
            return true;
        }

        let normalized = trimmed_line.trim_end_matches(';').trim().to_string();

        if !has_buffer && self.handle_db_refresh_request(&normalized, channel, session) {
            Handler::send_prompt(session, channel, self.current_prompt());
            ssh_probe.mark_success();
            return true;
        }

        if !has_buffer && Self::is_immediate_db_command(&normalized) {
            self.record_history(&normalized);
            return self.process_db_command(&normalized, channel, session, ssh_probe);
        }

        if !trimmed.is_empty() {
            if !self.db_multiline_buffer.is_empty() {
                self.db_multiline_buffer.push('\n');
            }
            self.db_multiline_buffer.push_str(raw_line.trim_end());
        }

        if trimmed.ends_with(';') {
            let statement = self.db_multiline_buffer.trim().to_string();
            self.db_multiline_buffer.clear();
            self.record_history(&statement);
            return self.process_db_command(&statement, channel, session, ssh_probe);
        }

        Handler::send_prompt(session, channel, self.current_prompt());
        ssh_probe.mark_success();
        true
    }

    fn process_db_command(
        &mut self,
        command: &str,
        channel: ChannelId,
        session: &mut Session,
        ssh_probe: &mut ServiceProbe,
    ) -> bool {
        let mut writer = SessionWriter::new(session, channel);
        let trimmed = command.trim();
        let mut db_probe = ServiceProbe::new(&self.services, "db-shell");
        if trimmed.is_empty() {
            Handler::send_prompt(session, channel, self.current_prompt());
            db_probe.mark_success();
            ssh_probe.mark_success();
            return true;
        }
        self.db_multiline_buffer.clear();

        if self.db_session.is_none() {
            if !self.services.db_shell.is_enabled() {
                let _ = writeln!(&mut writer, "{}", ssh_messages::db_shell_disabled_return());
                self.mode = ShellMode::Main;
                self.db_multiline_buffer.clear();
                Handler::send_prompt(session, channel, self.current_prompt());
                db_probe.mark_success();
                ssh_probe.mark_success();
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
                    let _ = writeln!(&mut writer, "{}", cli_shell_outcome::db_shell_error(&err));
                    self.services.registry().set_status(
                        "db-shell",
                        ServiceStatus::Degraded,
                        Some(format!("runtime error: {err}")),
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
                    "{}",
                    ssh_messages::db_shell_disabled_return_short()
                );
                self.mode = ShellMode::Main;
                self.db_multiline_buffer.clear();
                self.db_session = None;
                self.db_executor = None;
                Handler::send_prompt(session, channel, self.current_prompt());
                db_probe.mark_success();
                ssh_probe.mark_success();
                return true;
            }
            match db_shell::apply_command(db_session, trimmed, executor.as_ref(), &mut writer) {
                Ok(true) => {
                    // Refresh completion cache after DDL commands
                    let cmd_upper = trimmed.to_uppercase();
                    let needs_refresh = cmd_upper.contains("CREATE TABLE")
                        || cmd_upper.contains("DROP TABLE")
                        || cmd_upper.contains("ALTER TABLE")
                        || cmd_upper.contains("CREATE INDEX")
                        || cmd_upper.contains("DROP INDEX")
                        || cmd_upper.contains("CREATE VIEW")
                        || cmd_upper.contains("DROP VIEW");
                    if needs_refresh {
                        let mut sink = std::io::sink();
                        let DbCompletionCatalog {
                            entries,
                            table_names,
                        } = build_completion_list_with_tables(
                            db_session,
                            executor.as_ref(),
                            &mut sink,
                        );
                        self.db_completion_tables = table_names.iter().cloned().collect();
                        self.db_completion_words = entries;
                    }
                    Handler::send_prompt(session, channel, self.current_prompt());
                    db_probe.mark_success();
                    ssh_probe.mark_success();
                    true
                }
                Ok(false) => {
                    self.mode = ShellMode::Main;
                    self.history_index = None;
                    self.db_multiline_buffer.clear();
                    self.db_session = None;
                    self.db_executor = None;
                    self.services.registry().set_status(
                        "db-shell",
                        ServiceStatus::Active,
                        Some(cli_shell_outcome::DB_SHELL_READY_NOTE.to_string()),
                    );
                    Handler::send_prompt(session, channel, self.current_prompt());
                    db_probe.mark_success();
                    ssh_probe.mark_success();
                    true
                }
                Err(err) => {
                    tracing::error!(error = %err, "db-shell command failed over ssh");
                    let _ = writeln!(&mut writer, "{}", cli_shell_outcome::db_shell_error(&err));
                    self.services.registry().set_status(
                        "db-shell",
                        ServiceStatus::Degraded,
                        Some(cli_shell_outcome::db_shell_failure_note(&err).to_string()),
                    );
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
            }
        } else {
            let _ = writeln!(&mut writer, "{}", ssh_messages::db_shell_not_initialized());
            Handler::send_prompt(session, channel, self.current_prompt());
            db_probe.mark_success();
            ssh_probe.mark_success();
            true
        }
    }

    fn handle_db_refresh_request(
        &mut self,
        normalized: &str,
        channel: ChannelId,
        session: &mut Session,
    ) -> bool {
        if !Self::is_refresh_command(normalized) {
            return false;
        }
        if let (Some(db_session), Some(executor)) =
            (self.db_session.as_ref(), self.db_executor.as_ref())
        {
            let mut writer = SessionWriter::new(session, channel);
            let DbCompletionCatalog {
                entries,
                table_names,
            } = build_completion_list_with_tables(db_session, executor.as_ref(), &mut writer);
            let table_count = table_names.len();
            self.db_completion_tables = table_names.iter().cloned().collect();
            self.db_completion_words = entries;
            let _ = writeln!(&mut writer, "completion refreshed: {} tables", table_count);
        } else {
            let mut writer = SessionWriter::new(session, channel);
            let _ = writeln!(
                &mut writer,
                "completion refresh unavailable (session not ready)"
            );
        }
        true
    }

    fn is_refresh_command(command: &str) -> bool {
        command.eq_ignore_ascii_case(r"\refresh")
            || command.eq_ignore_ascii_case("/refresh")
            || command.eq_ignore_ascii_case("refresh")
    }

    fn is_immediate_db_command(command: &str) -> bool {
        if command.is_empty() {
            return false;
        }
        let lower = command.to_ascii_lowercase();
        if matches!(lower.as_str(), "help" | "exit" | "quit") {
            return true;
        }
        if command.starts_with('\\') {
            return true;
        }
        if command.starts_with('/') && !command.contains(' ') {
            return true;
        }
        false
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
            let _ = writeln!(
                &mut writer,
                "{}",
                ssh_messages::completion_hidden_hint(hidden)
            );
        }
    }

    fn complete_db(&mut self, channel: ChannelId, session: &mut Session) {
        // Use cached completion words (includes table names)
        let commands = &self.db_completion_words;
        if commands.is_empty() {
            session.data(channel, CryptoVec::from_slice(b"\x07"));
            return;
        }
        let (start, matches) = if self.db_completion_tables.is_empty() {
            completion::completion_matches(commands, &self.buffer, self.cursor)
        } else {
            completion::db_shell_completion_matches(
                commands,
                &self.db_completion_tables,
                &self.buffer,
                self.cursor,
            )
        };
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
            // Display matches in grid format (horizontal columns) like main CLI
            let total = matches.len();
            let hidden = total.saturating_sub(COMPLETION_MAX_VISIBLE);
            let visible: Vec<_> = matches.iter().take(COMPLETION_MAX_VISIBLE).collect();

            let max_len = visible.iter().map(|s| s.len()).max().unwrap_or(0);
            let col_width = max_len
                .saturating_add(COMPLETION_PADDING)
                .max(COMPLETION_PADDING);
            let cols = (COMPLETION_DISPLAY_WIDTH / col_width)
                .max(1)
                .min(visible.len());

            for chunk in visible.chunks(cols) {
                for (i, entry) in chunk.iter().enumerate() {
                    if cols == 1 || i + 1 == chunk.len() {
                        let _ = write!(&mut writer, "{entry}");
                    } else {
                        let _ = write!(&mut writer, "{entry:<width$}", width = col_width);
                    }
                }
                let _ = writeln!(&mut writer);
            }

            if hidden > 0 {
                let _ = writeln!(
                    &mut writer,
                    "{}",
                    ssh_messages::completion_hidden_hint(hidden)
                );
            }
        }
        self.render_buffer(session, channel);
    }

    fn current_prompt(&self) -> &str {
        match self.mode {
            ShellMode::Main => &self.main_prompt,
            ShellMode::DbShell => {
                if self.db_multiline_buffer.is_empty() {
                    &self.db_prompt
                } else {
                    &self.db_continuation_prompt
                }
            }
        }
    }

    /// Returns the 0-based line index the cursor currently sits on.
    fn cursor_line(&self) -> usize {
        self.buffer[..self.cursor].matches('\n').count()
    }

    /// Returns the total number of lines in the buffer (0-based last index).
    fn buffer_last_line(&self) -> usize {
        self.buffer.matches('\n').count()
    }

    /// Move cursor one line up inside a multi-line buffer, keeping roughly
    /// the same column. Returns `true` if the cursor actually moved.
    fn move_cursor_up_in_buffer(&mut self, session: &mut Session, channel: ChannelId) -> bool {
        if !self.buffer.contains('\n') {
            return false;
        }
        let cur_line = self.cursor_line();
        if cur_line == 0 {
            return false; // already on first line
        }

        // Column on the current line
        let before = &self.buffer[..self.cursor];
        let col = before
            .rfind('\n')
            .map(|p| self.cursor - p - 1)
            .unwrap_or(self.cursor);

        // Find the start of the previous line
        let cur_line_start = before.rfind('\n').unwrap(); // guaranteed since cur_line > 0
        let prev_content = &self.buffer[..cur_line_start];
        let prev_line_start = prev_content.rfind('\n').map(|p| p + 1).unwrap_or(0);
        let prev_line_len = cur_line_start - prev_line_start;

        self.cursor = prev_line_start + col.min(prev_line_len);
        self.render_buffer(session, channel);
        true
    }

    /// Move cursor one line down inside a multi-line buffer, keeping roughly
    /// the same column. Returns `true` if the cursor actually moved.
    fn move_cursor_down_in_buffer(&mut self, session: &mut Session, channel: ChannelId) -> bool {
        if !self.buffer.contains('\n') {
            return false;
        }
        let cur_line = self.cursor_line();
        let last_line = self.buffer_last_line();
        if cur_line >= last_line {
            return false; // already on last line
        }

        // Column on the current line
        let before = &self.buffer[..self.cursor];
        let col = before
            .rfind('\n')
            .map(|p| self.cursor - p - 1)
            .unwrap_or(self.cursor);

        // Find the start of the next line
        let next_line_start = self.buffer[self.cursor..].find('\n').unwrap() + self.cursor + 1;
        let next_line_end = self.buffer[next_line_start..]
            .find('\n')
            .map(|p| next_line_start + p)
            .unwrap_or(self.buffer.len());
        let next_line_len = next_line_end - next_line_start;

        self.cursor = next_line_start + col.min(next_line_len);
        self.render_buffer(session, channel);
        true
    }

    fn history_prev(&mut self, session: &mut Session, channel: ChannelId) {
        // If the buffer is multi-line and cursor is not on the first line,
        // move the cursor up instead of navigating history.
        if self.buffer.contains('\n') && self.cursor_line() > 0 {
            self.move_cursor_up_in_buffer(session, channel);
            return;
        }

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
        // If the buffer is multi-line and cursor is not on the last line,
        // move the cursor down instead of navigating history.
        if self.buffer.contains('\n') && self.cursor_line() < self.buffer_last_line() {
            self.move_cursor_down_in_buffer(session, channel);
            return;
        }

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

        let is_multiline = matches!(self.mode, ShellMode::DbShell) && self.buffer.contains('\n');

        // Move terminal cursor back to row 0 of the rendered block.
        // Use rendered_cursor_row (the actual row the terminal cursor is on).
        if self.rendered_cursor_row > 0 {
            let up = format!("\x1b[{}A", self.rendered_cursor_row);
            session.data(channel, CryptoVec::from_slice(up.as_bytes()));
        }

        if is_multiline {
            let lines: Vec<&str> = self.buffer.split('\n').collect();
            let prompt = self.current_prompt().to_string();
            let cont_prompt = DB_CONTINUATION_PROMPT;

            // Render each line with proper prompt
            for (i, line) in lines.iter().enumerate() {
                session.data(channel, CryptoVec::from_slice(b"\r\x1b[K"));
                let p = if i == 0 { prompt.as_str() } else { cont_prompt };
                Handler::send_prompt(session, channel, p);
                if !line.is_empty() {
                    session.data(channel, CryptoVec::from_slice(line.as_bytes()));
                }
                if i < lines.len() - 1 {
                    session.data(channel, CryptoVec::from_slice(b"\r\n"));
                }
            }

            // Clear any leftover lines from a previous longer render
            let new_count = lines.len();
            if self.rendered_line_count > new_count {
                let extra = self.rendered_line_count - new_count;
                for _ in 0..extra {
                    session.data(channel, CryptoVec::from_slice(b"\r\n\x1b[K"));
                }
                let up = format!("\x1b[{}A", extra);
                session.data(channel, CryptoVec::from_slice(up.as_bytes()));
            }
            self.rendered_line_count = new_count;

            // Position cursor correctly within the multi-line display.
            // After rendering, terminal cursor sits on the last content line.
            let before_cursor = &self.buffer[..self.cursor];
            let cursor_line = before_cursor.matches('\n').count();
            let cursor_col = before_cursor
                .rfind('\n')
                .map(|pos| before_cursor[pos + 1..].chars().count())
                .unwrap_or(before_cursor.chars().count());
            let last_line = lines.len() - 1;

            // Move up from the last rendered line to the cursor's line
            if last_line > cursor_line {
                let up = format!("\x1b[{}A", last_line - cursor_line);
                session.data(channel, CryptoVec::from_slice(up.as_bytes()));
            }

            // Move to the correct column (prompt width + text offset)
            let prompt_width = if cursor_line == 0 {
                visible_width(&prompt)
            } else {
                cont_prompt.len()
            };
            session.data(channel, CryptoVec::from_slice(b"\r"));
            let target_col = prompt_width + cursor_col;
            if target_col > 0 {
                let right = format!("\x1b[{}C", target_col);
                session.data(channel, CryptoVec::from_slice(right.as_bytes()));
            }

            // Track which row the terminal cursor actually ended up on
            self.rendered_cursor_row = cursor_line;
        } else {
            // Single-line rendering (original path)
            session.data(channel, CryptoVec::from_slice(b"\r\x1b[K"));
            Handler::send_prompt(session, channel, self.current_prompt());
            if !self.buffer.is_empty() {
                session.data(channel, CryptoVec::from_slice(self.buffer.as_bytes()));
            }

            // Clear leftover lines from a previous multi-line render.
            // \x1b[J erases from cursor to end of display without moving the cursor.
            if self.rendered_line_count > 1 {
                session.data(channel, CryptoVec::from_slice(b"\x1b[J"));
            }
            self.rendered_line_count = 1;
            self.rendered_cursor_row = 0;

            let tail_len = self.buffer[self.cursor..].chars().count();
            if tail_len > 0 {
                let seq = format!("\x1b[{}D", tail_len);
                session.data(channel, CryptoVec::from_slice(seq.as_bytes()));
            }
        }
    }

    fn handle_command_outcome(
        &mut self,
        outcome: CommandOutcome,
        channel: ChannelId,
        session: &mut Session,
    ) -> bool {
        match outcome {
            CommandOutcome::Continue => {
                Handler::send_prompt(session, channel, self.current_prompt());
                true
            }
            CommandOutcome::ExitShell => {
                session.close(channel);
                false
            }
            CommandOutcome::EnterDbShell => {
                if !self.services.db_shell.is_enabled() {
                    let mut writer = SessionWriter::new(session, channel);
                    let _ = writeln!(&mut writer, "{}", ssh_messages::db_shell_disabled_hint());
                    Handler::send_prompt(session, channel, self.current_prompt());
                    return true;
                }
                self.mode = ShellMode::DbShell;
                self.buffer.clear();
                self.history_index = None;
                self.cursor = 0;
                self.db_multiline_buffer.clear();
                let db_session = self.services.db_shell.create_session();
                let current_engine = db_session.current_engine();
                let engines = db_session
                    .available_engines()
                    .iter()
                    .map(|engine| engine.to_string())
                    .collect::<Vec<_>>();
                match RuntimeExecutor::new() {
                    Ok(executor) => {
                        let executor = Arc::new(executor);
                        // Load table names for completion
                        let mut sink = std::io::sink();
                        let DbCompletionCatalog {
                            entries,
                            table_names,
                        } = build_completion_list_with_tables(
                            &db_session,
                            executor.as_ref(),
                            &mut sink,
                        );
                        let table_count = table_names.len();
                        self.db_completion_tables = table_names.iter().cloned().collect();
                        self.db_completion_words = entries;
                        self.db_executor = Some(executor);
                        self.db_session = Some(db_session);
                        let mut writer = SessionWriter::new(session, channel);
                        let _ = writeln!(
                            &mut writer,
                            "{}",
                            ssh_messages::db_shell_active(current_engine)
                        );
                        if !engines.is_empty() {
                            let joined = engines.join(", ");
                            let _ = writeln!(
                                &mut writer,
                                "{}",
                                ssh_messages::db_shell_engines_list(&joined)
                            );
                        }
                        let _ = writeln!(
                            &mut writer,
                            "{}",
                            ssh_messages::db_shell_force_hint(DESTRUCTIVE_FORCE_WARNING)
                        );
                        if table_count > 0 {
                            let _ =
                                writeln!(&mut writer, "completion: {} tables loaded", table_count);
                        }
                        Handler::send_prompt(session, channel, &self.db_prompt);
                    }
                    Err(err) => {
                        self.mode = ShellMode::Main;
                        let mut writer = SessionWriter::new(session, channel);
                        let _ =
                            writeln!(&mut writer, "{}", ssh_messages::db_shell_start_failed(err));
                        Handler::send_prompt(session, channel, self.current_prompt());
                    }
                }
                true
            }
            CommandOutcome::AwaitConfirmation(request) => {
                self.begin_confirmation(request, channel, session);
                true
            }
            CommandOutcome::AsyncTask(task) => {
                let prompt = format!("\r\n{}", self.current_prompt());
                let output_sink = self.dependencies.output();
                let handle = session.handle();

                tokio::spawn(async move {
                    let result = task.await;
                    // Let any buffered module output flush first.
                    tokio::time::sleep(tokio::time::Duration::from_millis(75)).await;
                    match result {
                        Ok(_) => {
                            if let Some(sink) = output_sink {
                                sink.push(&prompt);
                                return;
                            }
                            let _ = handle
                                .data(channel, CryptoVec::from_slice(prompt.as_bytes()))
                                .await;
                        }
                        Err(err) => {
                            let error_text = ssh_messages::command_execution_error(&err);
                            if let Some(sink) = output_sink {
                                sink.push(&format!("\r\n{}{prompt}", error_text));
                                return;
                            }
                            let payload = format!("{}{prompt}", error_text);
                            let _ = handle
                                .data(channel, CryptoVec::from_slice(payload.as_bytes()))
                                .await;
                        }
                    }
                });
                true
            }
        }
    }

    fn handle_confirmation_input(
        &mut self,
        ch: char,
        channel: ChannelId,
        session: &mut Session,
    ) -> bool {
        match ch {
            '\u{1b}' => {
                self.escape_state = EscapeState::Esc;
                return true;
            }
            '\r' => {
                self.skip_next_lf = true;
                session.data(channel, CryptoVec::from_slice(b"\r\n"));
                return self.finish_confirmation(channel, session);
            }
            '\n' => {
                session.data(channel, CryptoVec::from_slice(b"\r\n"));
                return self.finish_confirmation(channel, session);
            }
            '\u{3}' => {
                self.buffer.clear();
                self.cursor = 0;
                session.data(channel, CryptoVec::from_slice(b"^C\r\n"));
                if let Some(request) = self.pending_confirmation.take() {
                    let mut writer = SessionWriter::new(session, channel);
                    match request.resolve(false, &self.dependencies, &mut writer) {
                        Ok(outcome) => {
                            return self.handle_command_outcome(outcome, channel, session)
                        }
                        Err(err) => {
                            let _ = writeln!(
                                &mut writer,
                                "{}",
                                cli_shell_runner::confirmation_failed(err)
                            );
                            Handler::send_prompt(session, channel, self.current_prompt());
                            return true;
                        }
                    }
                }
                return true;
            }
            '\u{8}' | '\u{7f}' => {
                if self.buffer.pop().is_some() {
                    self.cursor = self.buffer.len();
                    session.data(channel, CryptoVec::from_slice(b"\x08 \x08"));
                } else {
                    session.data(channel, CryptoVec::from_slice(b"\x07"));
                }
                return true;
            }
            '[' => {
                if matches!(self.escape_state, EscapeState::Esc) {
                    self.escape_state = EscapeState::Csi;
                    return true;
                }
            }
            ch if matches!(self.escape_state, EscapeState::Csi) => {
                if ('@'..='~').contains(&ch) {
                    self.escape_state = EscapeState::None;
                }
                return true;
            }
            _ => {
                self.escape_state = EscapeState::None;
            }
        }

        if ch.is_control() {
            return true;
        }

        let mut buf = [0u8; 4];
        let encoded = ch.encode_utf8(&mut buf);
        self.buffer.push_str(encoded);
        self.cursor = self.buffer.len();
        session.data(channel, CryptoVec::from_slice(encoded.as_bytes()));
        true
    }

    fn finish_confirmation(&mut self, channel: ChannelId, session: &mut Session) -> bool {
        let input = self.buffer.trim().to_ascii_lowercase();
        self.buffer.clear();
        self.cursor = 0;
        if input.is_empty() {
            self.render_confirmation_prompt(session, channel);
            return true;
        }

        if let Some(answer) = parse_confirmation_answer(&input) {
            let Some(request) = self.pending_confirmation.take() else {
                Handler::send_prompt(session, channel, self.current_prompt());
                return true;
            };

            // Create output sink for confirmation handling
            let output = Arc::new(SshCommandOutput::new(session.handle(), channel));
            let deps_with_output = self.dependencies.with_output(output);

            let mut writer = SessionWriter::new(session, channel);
            match request.resolve(answer, &deps_with_output, &mut writer) {
                Ok(outcome) => self.handle_command_outcome(outcome, channel, session),
                Err(err) => {
                    let _ = writeln!(
                        &mut writer,
                        "{}",
                        cli_shell_runner::confirmation_failed(err)
                    );
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
            }
        } else {
            let mut writer = SessionWriter::new(session, channel);
            let _ = writeln!(&mut writer, "{}", cli_shell_runner::CONFIRMATION_RETRY);
            self.render_confirmation_prompt(session, channel);
            true
        }
    }

    fn begin_confirmation(
        &mut self,
        request: ConfirmationRequest,
        channel: ChannelId,
        session: &mut Session,
    ) {
        self.pending_confirmation = Some(request);
        self.buffer.clear();
        self.cursor = 0;
        self.render_confirmation_prompt(session, channel);
    }

    fn render_confirmation_prompt(&mut self, session: &mut Session, channel: ChannelId) {
        if let Some(request) = self.pending_confirmation.as_ref() {
            Handler::send_prompt(session, channel, request.prompt());
        }
    }

    fn move_cursor_left(&mut self, session: &mut Session, channel: ChannelId) {
        if let Some(prev) = self.buffer[..self.cursor].chars().next_back() {
            self.cursor = self.cursor.saturating_sub(prev.len_utf8());
            if self.buffer.contains('\n') {
                self.render_buffer(session, channel);
            } else {
                session.data(channel, CryptoVec::from_slice(b"\x1b[D"));
            }
        }
    }

    fn move_cursor_right(&mut self, session: &mut Session, channel: ChannelId) {
        if let Some(next) = self.buffer[self.cursor..].chars().next() {
            self.cursor = (self.cursor + next.len_utf8()).min(self.buffer.len());
            if self.buffer.contains('\n') {
                self.render_buffer(session, channel);
            } else {
                session.data(channel, CryptoVec::from_slice(b"\x1b[C"));
            }
        }
    }

    /// Move the terminal cursor to the last rendered line of a multi-line display.
    /// This must be called before Enter/Ctrl+C so the next output appears below all
    /// rendered lines, not in the middle of them.
    fn move_to_last_rendered_line(&self, session: &mut Session, channel: ChannelId) {
        if self.rendered_line_count > 1 {
            let last_row = self.rendered_line_count - 1;
            if last_row > self.rendered_cursor_row {
                let down = format!("\x1b[{}B", last_row - self.rendered_cursor_row);
                session.data(channel, CryptoVec::from_slice(down.as_bytes()));
            }
        }
    }

    fn record_history(&mut self, command: &str) {
        if command.is_empty() {
            return;
        }
        if self.history.last().is_some_and(|last| last == command) {
            return;
        }
        if self.history.len() >= HISTORY_MAX {
            self.history.remove(0);
        }
        self.history.push(command.to_string());
    }
}

pub async fn start(cfg: &Arc<AppConfig>, services: &Arc<AppServices>) -> Result<()> {
    use russh::MethodSet;

    let mut base_config = server::Config {
        auth_rejection_time: Duration::from_secs(1),
        // Enable password authentication
        methods: MethodSet::PASSWORD,
        ..Default::default()
    };
    let host_key = load_or_create_host_key(&cfg.server.ssh.host_key_path)
        .with_context(|| format!("loading ssh host key from {}", cfg.server.ssh.host_key_path))?;
    let host_key_path = cfg.server.ssh.host_key_path.clone();
    base_config.keys.push(host_key.clone());
    if let Some(idle_seconds) = cfg.server.ssh.idle_close_seconds {
        base_config.inactivity_timeout = Some(Duration::from_secs(idle_seconds));
    }
    apply_allowed_ciphers(&mut base_config, &cfg.server.ssh.tls.allowed_ciphers);
    let base_config = Arc::new(base_config);

    struct Factory {
        username: String,
        password_env: Option<String>,
        identity_required: bool,
        config: Arc<AppConfig>,
        services: Arc<AppServices>,
    }
    impl Server for Factory {
        type Handler = Handler;
        fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
            // Track SSH connection count
            telemetry::increment_counter("ssh.connections.active", 1);
            telemetry::record_counter("ssh.connections.total", 1);

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
            let mut handler = Handler {
                username: self.username.clone(),
                password_env: self.password_env.clone(),
                identity_required: self.identity_required,
                identity_profile: None,
                role_label: SSH_DEFAULT_ROLE.to_string(),
                buffer: String::new(),
                main_prompt: String::new(),
                db_prompt: String::new(),
                db_continuation_prompt: DB_CONTINUATION_PROMPT.to_string(),
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
                db_completion_words: Vec::new(),
                db_completion_tables: HashSet::new(),
                db_multiline_buffer: String::new(),
                cursor: 0,
                rendered_line_count: 1,
                rendered_cursor_row: 0,
                pending_confirmation: None,
                channel: None,
            };
            handler.refresh_prompts();
            handler
        }
    }

    tracing::info!(host = %cfg.server.ssh.host, port = cfg.server.ssh.port, "starting SSH server");
    let bind_addr = format!("{}:{}", cfg.server.ssh.host, cfg.server.ssh.port);
    services.registry().set_status(
        "ssh-server",
        ServiceStatus::Active,
        Some(format!("listening on {bind_addr}")),
    );

    let identity_required = should_enforce_identity(cfg.as_ref());
    let mut server = Factory {
        username: cfg.server.ssh.user.clone(),
        password_env: if identity_required {
            None
        } else {
            Some(SSH_PASSWORD_ENV.to_string())
        },
        identity_required,
        config: Arc::clone(cfg),
        services: Arc::clone(services),
    };
    if let Some(reload_seconds) = cfg.server.ssh.tls.host_key_reload_seconds {
        let mut cached_host_key = host_key;
        let reload_interval = Duration::from_secs(reload_seconds);
        let mut last_reload = Instant::now();
        let listener = TcpListener::bind(&bind_addr).await?;
        let (error_tx, mut error_rx) = mpsc::unbounded_channel();
        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((socket, _addr)) => {
                            if last_reload.elapsed() >= reload_interval {
                                match load_or_create_host_key(&host_key_path) {
                                    Ok(key) => {
                                        cached_host_key = key;
                                        last_reload = Instant::now();
                                    }
                                    Err(err) => {
                                        tracing::warn!(
                                            path = %host_key_path,
                                            error = %err,
                                            "failed to reload SSH host key"
                                        );
                                    }
                                }
                            }
                            let mut config = build_server_config(cfg, cached_host_key.clone());
                            apply_allowed_ciphers(&mut config, &cfg.server.ssh.tls.allowed_ciphers);
                            let config = Arc::new(config);
                            let handler = server.new_client(socket.peer_addr().ok());
                            let error_tx = error_tx.clone();
                            tokio::spawn(async move {
                                let session = match server::run_stream(config, socket, handler).await {
                                    Ok(session) => session,
                                    Err(err) => {
                                        let _ = error_tx.send(err);
                                        return;
                                    }
                                };
                                if let Err(err) = session.await {
                                    let _ = error_tx.send(err);
                                }
                            });
                        }
                        _ => break,
                    }
                }
                Some(error) = error_rx.recv() => {
                    server.handle_session_error(error);
                }
            }
        }
        services.registry().set_status(
            "ssh-server",
            ServiceStatus::Stopped,
            Some("listener stopped".to_string()),
        );
        Ok(())
    } else {
        match server.run_on_address(base_config, &bind_addr).await {
            Ok(()) => {
                services.registry().set_status(
                    "ssh-server",
                    ServiceStatus::Stopped,
                    Some("listener stopped".to_string()),
                );
                Ok(())
            }
            Err(err) => {
                services.registry().set_status(
                    "ssh-server",
                    ServiceStatus::Failed,
                    Some(ssh_messages::service_failure_note(err.to_string())),
                );
                Err(err.into())
            }
        }
    }
}

fn should_enforce_identity(cfg: &AppConfig) -> bool {
    match cfg.security.identity.provider {
        // Embedded provider: always use identity (supports first-time password setup)
        IdentityProviderKind::Embedded => true,
        // External provider: only enforce in production with version >= 1.0.0
        IdentityProviderKind::External => {
            let environment = cfg.security.identity.environment.to_ascii_lowercase();
            let is_prod_env = matches!(environment.as_str(), "prod" | "production");
            if !is_prod_env {
                return false;
            }
            match Version::parse(&cfg.app.version) {
                Ok(version) => version >= Version::new(1, 0, 0),
                Err(_) => false,
            }
        }
    }
}

fn load_or_create_host_key(path: &str) -> Result<russh_keys::key::KeyPair> {
    let key_path = Path::new(path);
    if key_path.exists() {
        // Try to load as OpenSSH format
        match russh_keys::load_secret_key(key_path, None) {
            Ok(key) => return Ok(key),
            Err(e) => {
                tracing::warn!(
                    path = %key_path.display(),
                    error = %e,
                    "failed to load host key, regenerating"
                );
            }
        }
    }

    if let Some(parent) = key_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let key = russh_keys::key::KeyPair::generate_ed25519()
        .ok_or_else(|| anyhow!("failed to generate ed25519 host key"))?;

    // Save the key in PKCS8 PEM format
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
    russh_keys::encode_pkcs8_pem(&key, &mut file)
        .map_err(|e| anyhow!("failed to write host key: {}", e))?;
    tracing::info!(path = %key_path.display(), "generated new SSH host key");

    Ok(key)
}

fn build_server_config(cfg: &AppConfig, host_key: russh_keys::key::KeyPair) -> server::Config {
    use russh::MethodSet;

    let mut config = server::Config {
        auth_rejection_time: Duration::from_secs(1),
        methods: MethodSet::PASSWORD,
        ..Default::default()
    };
    config.keys.push(host_key);
    if let Some(idle_seconds) = cfg.server.ssh.idle_close_seconds {
        config.inactivity_timeout = Some(Duration::from_secs(idle_seconds));
    }
    config
}

fn apply_allowed_ciphers(config: &mut server::Config, allowed: &[String]) {
    let resolved = resolve_allowed_ciphers(allowed);
    if let Some(cipher_list) = resolved {
        let mut preferred = Preferred::DEFAULT;
        preferred.cipher = Cow::Owned(cipher_list);
        config.preferred = preferred;
    }
}

fn resolve_allowed_ciphers(allowed: &[String]) -> Option<Vec<cipher::Name>> {
    if allowed.is_empty() {
        return None;
    }
    let mut resolved = Vec::new();
    for cipher_name in allowed {
        let normalized = cipher_name.trim().to_ascii_lowercase();
        let cipher = match normalized.as_str() {
            "chacha20-poly1305" | "chacha20-poly1305@openssh.com" => cipher::CHACHA20_POLY1305,
            "aes256-gcm" | "aes-256-gcm" | "aes_256_gcm" | "aes256-gcm@openssh.com" => {
                cipher::AES_256_GCM
            }
            "aes256-ctr" | "aes-256-ctr" | "aes_256_ctr" => cipher::AES_256_CTR,
            "aes192-ctr" | "aes-192-ctr" | "aes_192_ctr" => cipher::AES_192_CTR,
            "aes128-ctr" | "aes-128-ctr" | "aes_128_ctr" => cipher::AES_128_CTR,
            _ => {
                tracing::warn!(
                    cipher = %cipher_name,
                    "unknown SSH cipher in server.ssh.tls.allowed_ciphers"
                );
                continue;
            }
        };
        if !resolved.contains(&cipher) {
            resolved.push(cipher);
        }
    }
    if resolved.is_empty() {
        None
    } else {
        Some(resolved)
    }
}

#[derive(Clone)]
struct SshCommandOutput {
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

impl SshCommandOutput {
    fn new(handle: SessionHandle, channel: ChannelId) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let runtime = TokioHandle::current();
        runtime.spawn(async move {
            while let Some(buf) = rx.recv().await {
                let mut payload = CryptoVec::new();
                payload.extend(&buf);
                let _ = handle.data(channel, payload).await;
            }
        });
        Self { tx }
    }
}

impl CommandOutput for SshCommandOutput {
    fn push(&self, text: &str) {
        let mut converted = Vec::with_capacity(text.len() * 2);
        for &byte in text.as_bytes() {
            if byte == b'\n' {
                converted.push(b'\r');
                converted.push(b'\n');
            } else {
                converted.push(byte);
            }
        }
        let _ = self.tx.send(converted);
    }
}
