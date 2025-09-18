use anyhow::Result;
use std::sync::Arc;
use thrussh::server::{Auth, Session, Server};
use thrussh::{server, ChannelId, CryptoVec};

use crate::config::AppConfig;

#[derive(Clone)]
struct Handler {
    username: String,
    password_env: String,
    buffer: String,
    server_name: String,
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
            "\n\x1b[32m{}\\x1b[0m\nAuthorized use only. Type 'exit' to disconnect.\n> ",
            self.server_name
        );
        session.data(channel, CryptoVec::from_slice(banner.as_bytes()));
        self.finished(session)
    }

    fn data(mut self, channel: ChannelId, data: &[u8], mut session: Session) -> Self::FutureUnit {
        self.buffer.push_str(&String::from_utf8_lossy(data));
        while let Some(pos) = self.buffer.find('\n') {
            let mut line = self.buffer.drain(..=pos).collect::<String>();
            if line.ends_with('\n') { line.pop(); }
            if line.ends_with('\r') { line.pop(); }
            let cmd = line.trim();
            match cmd {
                "help" => { session.data(channel, CryptoVec::from_slice(b"available: help, exit\n> ")); }
                "exit" => { session.data(channel, CryptoVec::from_slice(b"bye\n")); session.close(channel); return self.finished(session); }
                "" => { session.data(channel, CryptoVec::from_slice(b"> ")); }
                _ => { session.data(channel, CryptoVec::from_slice(b"command disabled on this endpoint\n> ")); }
            }
        }
        self.finished(session)
    }
}

pub async fn start(cfg: &AppConfig) -> Result<()> {
    let config = server::Config { auth_rejection_time: std::time::Duration::from_secs(1), ..Default::default() };
    let mut config = config;
    let key: thrussh_keys::key::KeyPair = thrussh_keys::key::KeyPair::generate_ed25519().unwrap();
    config.keys.push(key);
    let config = Arc::new(config);

    struct Factory { username: String, password_env: String, server_name: String }
    impl Server for Factory {
        type Handler = Handler;
        fn new(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
            Handler {
                username: self.username.clone(),
                password_env: self.password_env.clone(),
                buffer: String::new(),
                server_name: self.server_name.clone(),
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
