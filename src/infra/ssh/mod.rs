use anyhow::Result;
use std::sync::Arc;
use thrussh::server::{Auth, Server, Session};
use thrussh::{server, ChannelId, CryptoVec};

use crate::config::AppConfig;

#[derive(Clone)]
struct Handler {
    username: String,
    password_env: String,
    buffer: String,
    server_name: String,
    skip_next_lf: bool,
    escape_state: EscapeState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EscapeState {
    #[default]
    None,
    Esc,
    Csi,
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
        // Colorized server name (ANSI green) and formatted banner
        let banner = format!(
            "\r\n\x1b[32m{}\x1b[0m\r\nAuthorized use only. Type 'exit' to disconnect.\r\n",
            self.server_name
        );
        session.data(channel, CryptoVec::from_slice(banner.as_bytes()));
        Handler::send_prompt(&mut session, channel);
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
                    Handler::send_prompt(&mut session, channel);
                }
                ch => {
                    if ch.is_control() {
                        continue;
                    }
                    let mut buf = [0u8; 4];
                    let encoded = ch.encode_utf8(&mut buf);
                    self.buffer.push_str(encoded);
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

        if cmd.is_empty() {
            Handler::send_prompt(session, channel);
            return true;
        }

        match cmd.as_str() {
            "help" => {
                Handler::send_line(session, channel, "available: help, exit");
                Handler::send_prompt(session, channel);
                true
            }
            "exit" => {
                Handler::send_line(session, channel, "bye");
                session.close(channel);
                false
            }
            _ => {
                Handler::send_line(session, channel, "command disabled on this endpoint");
                Handler::send_prompt(session, channel);
                true
            }
        }
    }

    fn send_prompt(session: &mut Session, channel: ChannelId) {
        session.data(channel, CryptoVec::from_slice(b"> "));
    }

    fn send_line(session: &mut Session, channel: ChannelId, message: &str) {
        let mut buf = String::with_capacity(message.len() + 2);
        buf.push_str(message);
        buf.push_str("\r\n");
        session.data(channel, CryptoVec::from_slice(buf.as_bytes()));
    }
}

pub async fn start(cfg: &AppConfig) -> Result<()> {
    let config = server::Config {
        auth_rejection_time: std::time::Duration::from_secs(1),
        ..Default::default()
    };
    let mut config = config;
    let key: thrussh_keys::key::KeyPair = thrussh_keys::key::KeyPair::generate_ed25519().unwrap();
    config.keys.push(key);
    let config = Arc::new(config);

    struct Factory {
        username: String,
        password_env: String,
        server_name: String,
    }
    impl Server for Factory {
        type Handler = Handler;
        fn new(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
            Handler {
                username: self.username.clone(),
                password_env: self.password_env.clone(),
                buffer: String::new(),
                server_name: self.server_name.clone(),
                skip_next_lf: false,
                escape_state: EscapeState::None,
            }
        }
    }

    tracing::info!(host = %cfg.server.ssh.host, port = cfg.server.ssh.port, "starting SSH server");
    let bind_addr = format!("{}:{}", cfg.server.ssh.host, cfg.server.ssh.port);
    let server = Factory {
        username: cfg.server.ssh.user.clone(),
        password_env: "FENRIR_SSH_PASSWORD".to_string(),
        server_name: cfg.server.ssh.server_name.clone(),
    };
    thrussh::server::run(config, &bind_addr, server).await?;
    Ok(())
}
