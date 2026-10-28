//! The operating mode (W9A-03): one switch, `FENRIR_ENV`.
//!
//! - `prod` | `production` => production;
//! - `dev` | `development` | `local` | `test` => development;
//! - missing, empty or anything else => production (fail-safe).
//!
//! In production the host refuses to start while dev-only settings are
//! active (see [`production_violations`]), ignores `config/local.toml`, and
//! hands the mode to every module as `FENRIR_ENV=prod` (`dev` otherwise).

use std::net::IpAddr;

use super::model::{AppConfig, DbAuthMethod, DbRuntimeMode, IdentityProviderKind};

pub const ENV_OPERATING_MODE: &str = "FENRIR_ENV";

/// Shortest control token accepted in production (bytes).
const PRODUCTION_MIN_CONTROL_TOKEN_LEN: usize = 32;
/// The production password policy floor (`PasswordPolicy::standard`).
const PRODUCTION_MIN_PASSWORD_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OperatingMode {
    #[default]
    Production,
    Development,
}

impl OperatingMode {
    /// Parses a `FENRIR_ENV` value; `None`, empty and unknown are production.
    pub fn from_env_value(raw: Option<&str>) -> Self {
        match raw
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("dev" | "development" | "local" | "test") => Self::Development,
            _ => Self::Production,
        }
    }

    /// The mode of this process, from `FENRIR_ENV`.
    pub fn from_env() -> Self {
        Self::from_env_value(std::env::var(ENV_OPERATING_MODE).ok().as_deref())
    }

    pub const fn is_production(self) -> bool {
        matches!(self, Self::Production)
    }

    /// The value modules receive as `FENRIR_ENV`.
    pub const fn as_env_value(self) -> &'static str {
        match self {
            Self::Production => "prod",
            Self::Development => "dev",
        }
    }
}

/// Dev-only or insecure settings that are active in `cfg`, each as a short
/// sentence naming the key. Empty means the config may run in production.
pub fn production_violations(cfg: &AppConfig) -> Vec<String> {
    let mut found = Vec::new();
    let mut flag = |active: bool, message: &str| {
        if active {
            found.push(message.to_string());
        }
    };

    // Certificate checks switched off.
    flag(
        cfg.modules.registry.tls.accept_invalid_certs,
        "modules.registry.tls.accept_invalid_certs = true",
    );
    flag(
        cfg.modules.runtime.clients.tls.accept_invalid_certs,
        "modules.runtime.clients.tls.accept_invalid_certs = true",
    );
    flag(
        cfg.security.identity.external.tls.accept_invalid_certs,
        "security.identity.external.tls.accept_invalid_certs = true",
    );

    // Dev checkouts as module sources: the host would sync builds and run
    // migrations from them.
    flag(
        cfg.modules
            .dev_sources
            .base_path
            .as_deref()
            .is_some_and(|path| !path.trim().is_empty()),
        "modules.dev_sources.base_path (or FENRIR_DEV_MODULE_PATH) is set: dev module sources",
    );

    // Identity tokens minted for a dev environment.
    let identity_env = cfg
        .security
        .identity
        .environment
        .trim()
        .to_ascii_lowercase();
    flag(
        !matches!(identity_env.as_str(), "prod" | "production"),
        "security.identity.environment is not prod",
    );
    flag(
        cfg.security.identity.provider == IdentityProviderKind::External
            && cfg
                .security
                .identity
                .external
                .base_url
                .as_deref()
                .is_some_and(|url| !url.trim().to_ascii_lowercase().starts_with("https://")),
        "security.identity.external.base_url is not https://",
    );

    // Relaxed password policy (first-time setup of control-plane admins).
    let policy = &cfg.security.password_policy;
    flag(
        policy.min_length < PRODUCTION_MIN_PASSWORD_LEN,
        "security.password_policy.min_length is below 12",
    );
    flag(
        !policy.check_common_passwords,
        "security.password_policy.check_common_passwords = false",
    );

    // Control tokens written into a config file or too short to be random.
    for entry in &cfg.security.http.control_tokens {
        let reference = entry.split_once(':').map_or("", |(_, value)| value.trim());
        match reference.strip_prefix("env:") {
            None => flag(
                true,
                "security.http.control_tokens contains an inline token (use env:VAR)",
            ),
            Some(var) => {
                let len = std::env::var(var.trim()).map_or(0, |value| value.trim().len());
                // A `_FILE` reference was read and checked by `validate`;
                // only a direct value is measured here.
                flag(
                    len > 0 && len < PRODUCTION_MIN_CONTROL_TOKEN_LEN,
                    "security.http.control_tokens holds a token shorter than 32 characters",
                );
            }
        }
    }

    // Unauthenticated local database.
    flag(
        cfg.db.runtime.mode == DbRuntimeMode::Embedded
            && cfg.db.runtime.embedded.security.auth_method == DbAuthMethod::Trust,
        "db.runtime.embedded.security.auth_method = \"trust\"",
    );

    // Plain-text listeners reachable from outside the machine.
    flag(
        cfg.server.enable_http
            && !cfg.server.http.tls.enabled
            && !is_loopback_host(&cfg.server.http.host),
        "server.http listens beyond loopback without TLS",
    );
    flag(
        cfg.server.enable_grpc
            && cfg
                .server
                .grpc
                .as_ref()
                .is_some_and(|grpc| !grpc.tls.enabled && !is_loopback_host(&grpc.host)),
        "server.grpc listens beyond loopback without TLS",
    );
    // The SSH admin shell runs on russh 0.44 (known pre-auth DoS, see
    // deny.toml) until the upgrade: loopback only.
    flag(
        !is_loopback_host(&cfg.server.ssh.host),
        "server.ssh listens beyond loopback",
    );
    flag(
        is_plain_remote_url(&cfg.modules.registry.url),
        "modules.registry.url is plain http:// to a remote host",
    );

    found
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.to_canonical().is_loopback())
}

fn is_plain_remote_url(url: &str) -> bool {
    let Some(rest) = url.trim().strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if host.starts_with('[') {
        host.split(']').next().map_or(host, |inner| &inner[1..])
    } else {
        host.split(':').next().unwrap_or(host)
    };
    !is_loopback_host(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_values_follow_the_spec() {
        for value in ["prod", "production", " PROD ", "Production"] {
            assert_eq!(
                OperatingMode::from_env_value(Some(value)),
                OperatingMode::Production,
                "{value}"
            );
        }
        for value in ["dev", "development", "local", "test", "DEV"] {
            assert_eq!(
                OperatingMode::from_env_value(Some(value)),
                OperatingMode::Development,
                "{value}"
            );
        }
        for value in [None, Some(""), Some("staging"), Some("devel"), Some("qa")] {
            assert_eq!(
                OperatingMode::from_env_value(value),
                OperatingMode::Production,
                "{value:?}"
            );
        }
        assert_eq!(OperatingMode::default(), OperatingMode::Production);
        assert_eq!(OperatingMode::Production.as_env_value(), "prod");
        assert_eq!(OperatingMode::Development.as_env_value(), "dev");
    }

    #[test]
    fn the_forwarded_value_is_understood_by_the_same_rules() {
        for mode in [OperatingMode::Production, OperatingMode::Development] {
            assert_eq!(
                OperatingMode::from_env_value(Some(mode.as_env_value())),
                mode
            );
        }
    }

    #[test]
    fn loopback_and_remote_detection() {
        for host in ["127.0.0.1", "localhost", "::1", "[::1]", "127.1.2.3"] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in ["0.0.0.0", "10.0.0.1", "example.com", "::"] {
            assert!(!is_loopback_host(host), "{host}");
        }
        assert!(!is_plain_remote_url("http://localhost:8443"));
        assert!(!is_plain_remote_url("http://127.0.0.1:8443/api"));
        assert!(!is_plain_remote_url("http://[::1]:8443"));
        assert!(!is_plain_remote_url("https://registry.example"));
        assert!(is_plain_remote_url("http://registry.example:8443"));
        assert!(is_plain_remote_url("http://user@10.0.0.5/x"));
    }
}
