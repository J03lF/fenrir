use anyhow::{anyhow, Context, Result};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use thrussh::server::{Auth, Server, Session};
use thrussh::{server, ChannelId, CryptoVec};

use crate::cli::commands::builtins;
use crate::cli::commands::registry::{
    CommandOutcome, CommandRegistry, CommandStatus, ShellEnvironment,
};
use crate::config::AppConfig;
use crate::prompts;

const HISTORY_MAX: usize = 200;

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
    registry: CommandRegistry,
    history: Vec<String>,
    history_index: Option<usize>,
    mode: ShellMode,
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
                '\u{8}' | '\u{7f}' => {
                    if self.buffer.pop().is_some() {
                        session.data(channel, CryptoVec::from_slice(b"\x08 \x08"));
                    }
                }
                '\u{3}' => {
                    self.buffer.clear();
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
                    self.buffer.push_str(encoded);
                    self.history_index = None;
                    session.data(channel, CryptoVec::from_slice(encoded.as_bytes()));
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
            let mut writer = SessionWriter::new(session, channel);
            match self.registry.execute(
                name,
                &args,
                self.config.as_ref(),
                &mut writer,
                ShellEnvironment::Ssh,
            ) {
                Ok(CommandStatus::Executed(CommandOutcome::Continue)) => {
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
                }
                Ok(CommandStatus::Executed(CommandOutcome::ExitShell)) => {
                    session.close(channel);
                    false
                }
                Ok(CommandStatus::Executed(CommandOutcome::EnterDbShell)) => {
                    self.mode = ShellMode::DbShell;
                    self.buffer.clear();
                    self.history_index = None;
                    let _ = writeln!(&mut writer, "DB-Shell (Stub) aktiv. 'exit' kehrt zurück.");
                    Handler::send_prompt(session, channel, self.current_prompt());
                    true
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
        match command.as_str() {
            "exit" => {
                self.mode = ShellMode::Main;
                self.history_index = None;
                let _ = writeln!(&mut writer, "DB-Shell beendet");
                Handler::send_prompt(session, channel, self.current_prompt());
                true
            }
            "help" => {
                let _ = writeln!(&mut writer, "db-shell: stub. commands: help, exit");
                Handler::send_prompt(session, channel, self.current_prompt());
                true
            }
            other => {
                let _ = writeln!(
                    &mut writer,
                    "stub: received '{}' - keine Datenbank verbunden",
                    other
                );
                Handler::send_prompt(session, channel, self.current_prompt());
                true
            }
        }
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
        self.render_buffer(session, channel);
    }

    fn render_buffer(&self, session: &mut Session, channel: ChannelId) {
        session.data(channel, CryptoVec::from_slice(b"\r"));
        Handler::send_prompt(session, channel, self.current_prompt());
        session.data(channel, CryptoVec::from_slice(b"\x1b[K"));
        if !self.buffer.is_empty() {
            session.data(channel, CryptoVec::from_slice(self.buffer.as_bytes()));
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

pub async fn start(cfg: &AppConfig) -> Result<()> {
    let config = server::Config {
        auth_rejection_time: std::time::Duration::from_secs(1),
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
    }
    impl Server for Factory {
        type Handler = Handler;
        fn new(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
            let prompts = prompts::prompt_set(self.config.as_ref());
            Handler {
                username: self.username.clone(),
                password_env: self.password_env.clone(),
                buffer: String::new(),
                main_prompt: prompts.main_transport,
                db_prompt: prompts.db_transport,
                skip_next_lf: false,
                escape_state: EscapeState::None,
                config: Arc::clone(&self.config),
                registry: builtins::build_registry(),
                history: Vec::new(),
                history_index: None,
                mode: ShellMode::Main,
            }
        }
    }

    tracing::info!(host = %cfg.server.ssh.host, port = cfg.server.ssh.port, "starting SSH server");
    let bind_addr = format!("{}:{}", cfg.server.ssh.host, cfg.server.ssh.port);
    let server = Factory {
        username: cfg.server.ssh.user.clone(),
        password_env: "FENRIR_SSH_PASSWORD".to_string(),
        config: Arc::new(cfg.clone()),
    };
    thrussh::server::run(config, &bind_addr, server).await?;
    Ok(())
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
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
