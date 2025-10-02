use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub app: AppSection,
    pub server: ServerSection,
    pub security: SecuritySection,
    pub db: DbSection,
    pub telemetry: TelemetrySection,
    pub audit: AuditSection,
    pub cli: CliSection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppSection {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerSection {
    pub enable_http: bool,
    pub enable_grpc: bool,
    pub ssh: SshConfig,
    pub http: HttpConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub server_name: String,
    pub host_key_path: String,
    pub idle_close_seconds: Option<u64>,
    #[serde(default)]
    pub tls: SshTlsConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub tls: HttpTlsConfig,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct HttpTlsConfig {
    pub enabled: bool,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    #[serde(default)]
    pub cipher_suites: Vec<String>,
    pub reload_interval_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct SshTlsConfig {
    #[serde(default)]
    pub allowed_ciphers: Vec<String>,
    pub host_key_reload_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SecuritySection {
    pub kdf: KdfConfig,
    pub jwt: JwtConfig,
    pub allowed_ciphers: Vec<String>,
    #[serde(default)]
    pub http: HttpSecuritySection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct KdfConfig {
    pub algorithm: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct JwtConfig {
    pub issuer: String,
    pub audience: String,
    pub exp_seconds: u64,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct HttpSecuritySection {
    #[serde(default)]
    pub control_tokens: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct HttpControlToken {
    pub role: String,
    pub secret: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DbSection {
    pub default_engine: String,
    #[serde(default)]
    pub connections: DbConnections,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct DbConnections {
    pub postgres: Option<DbConnectionSettings>,
    pub mysql: Option<DbConnectionSettings>,
    pub sqlite: Option<DbConnectionSettings>,
    pub mongodb: Option<DbConnectionSettings>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DbConnectionSettings {
    pub uri: String,
    #[serde(default)]
    pub pool: DbPoolSettings,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct DbPoolSettings {
    pub max: Option<u32>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelemetrySection {
    pub tracing_level: String,
    pub metrics_enabled: bool,
    pub health_enabled: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuditSection {
    pub enabled: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CliSection {
    pub prompt_theme: String,
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("{0}")]
    Anyhow(#[from] anyhow::Error),
    #[error("invalid configuration: {0}")]
    Invalid(&'static str),
    #[error("missing environment variable {var} for {key}")]
    MissingEnv { key: &'static str, var: String },
}

impl HttpSecuritySection {
    pub fn resolve_control_tokens(&self) -> Result<Vec<HttpControlToken>, ConfigError> {
        let mut resolved = Vec::with_capacity(self.control_tokens.len());
        for entry in &self.control_tokens {
            let (role, value) = parse_role_token(entry)?;
            let secret = resolve_secret(value)?;
            if secret.is_empty() {
                return Err(ConfigError::Invalid(
                    "security.http.control_tokens secrets must not be empty",
                ));
            }
            resolved.push(HttpControlToken { role, secret });
        }
        Ok(resolved)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let tokens = self.resolve_control_tokens()?;
        for token in tokens {
            match token.role.as_str() {
                "admin" | "operator" | "viewer" => {}
                _ => {
                    return Err(ConfigError::Invalid(
                        "security.http.control_tokens must prefix role as admin|operator|viewer",
                    ))
                }
            }
        }
        Ok(())
    }
}

impl DbConnectionSettings {
    pub fn resolve_uri(&self, key: &'static str) -> Result<String, ConfigError> {
        let raw = self.uri.trim();
        if raw.is_empty() {
            return Err(ConfigError::Invalid("database uri must not be empty"));
        }
        if let Some(env) = raw.strip_prefix("env:") {
            let var = env.trim();
            match std::env::var(var) {
                Ok(value) if !value.trim().is_empty() => Ok(value),
                Ok(_) => Err(ConfigError::Invalid(
                    "database uri environment variable must not be empty",
                )),
                Err(_) => Err(ConfigError::MissingEnv {
                    key,
                    var: var.to_string(),
                }),
            }
        } else {
            Ok(raw.to_string())
        }
    }

    pub fn pool_max(&self) -> Option<u32> {
        self.pool.max
    }

    pub fn pool_timeout(&self) -> Option<u64> {
        self.pool.timeout_ms
    }
}

fn parse_role_token(raw: &str) -> Result<(String, &str), ConfigError> {
    let mut parts = raw.splitn(2, ':');
    let role = parts
        .next()
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .ok_or(ConfigError::Invalid(
            "security.http.control_tokens entries must start with <role>:<secret>",
        ))?;
    let value = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::Invalid(
            "security.http.control_tokens entries must contain a secret reference",
        ))?;
    Ok((role.to_ascii_lowercase(), value))
}

fn resolve_secret(value: &str) -> Result<String, ConfigError> {
    if let Some(env) = value.strip_prefix("env:") {
        let var = env.trim();
        match std::env::var(var) {
            Ok(secret) if !secret.trim().is_empty() => Ok(secret),
            Ok(_) => Err(ConfigError::Invalid(
                "environment variable for control token must not be empty",
            )),
            Err(_) => Err(ConfigError::MissingEnv {
                key: "security.http.control_tokens",
                var: var.to_string(),
            }),
        }
    } else {
        Ok(value.to_string())
    }
}

pub fn load() -> Result<AppConfig, ConfigError> {
    let mut builder =
        config::Config::builder().add_source(config::File::with_name("config/default.toml"));

    // Add env overrides like FENRIR__SERVER__SSH__PORT=2222
    builder = builder.add_source(config::Environment::with_prefix("FENRIR").separator("__"));

    let cfg: AppConfig = builder
        .build()
        .map_err(|e| ConfigError::Anyhow(e.into()))?
        .try_deserialize()
        .map_err(|e| ConfigError::Anyhow(e.into()))?;
    validate(&cfg)?;
    Ok(cfg)
}

pub fn validate(cfg: &AppConfig) -> Result<(), ConfigError> {
    if !matches!(
        cfg.db.default_engine.as_str(),
        "postgres" | "mysql" | "sqlite" | "mongodb"
    ) {
        return Err(ConfigError::Invalid(
            "db.default_engine must be one of postgres|mysql|sqlite|mongodb",
        ));
    }

    let connections = &cfg.db.connections;
    let default_key = match cfg.db.default_engine.as_str() {
        "postgres" => {
            let settings = connections.postgres.as_ref().ok_or(ConfigError::Invalid(
                "db.connections.postgres.uri must be set when postgres is default",
            ))?;
            settings.resolve_uri("db.connections.postgres.uri")?;
            settings
        }
        "mysql" => {
            let settings = connections.mysql.as_ref().ok_or(ConfigError::Invalid(
                "db.connections.mysql.uri must be set when mysql is default",
            ))?;
            settings.resolve_uri("db.connections.mysql.uri")?;
            settings
        }
        "sqlite" => {
            let settings = connections.sqlite.as_ref().ok_or(ConfigError::Invalid(
                "db.connections.sqlite.uri must be set when sqlite is default",
            ))?;
            settings.resolve_uri("db.connections.sqlite.uri")?;
            settings
        }
        "mongodb" => {
            let settings = connections.mongodb.as_ref().ok_or(ConfigError::Invalid(
                "db.connections.mongodb.uri must be set when mongodb is default",
            ))?;
            settings.resolve_uri("db.connections.mongodb.uri")?;
            settings
        }
        _ => unreachable!(),
    };

    if matches!(default_key.pool.max, Some(0)) {
        return Err(ConfigError::Invalid(
            "db pool max must be greater than zero",
        ));
    }
    if matches!(default_key.pool.timeout_ms, Some(0)) {
        return Err(ConfigError::Invalid(
            "db pool timeout must be greater than zero",
        ));
    }

    if cfg.server.http.port == 0 || cfg.server.ssh.port == 0 {
        return Err(ConfigError::Invalid("server ports must be > 0"));
    }
    if cfg.security.jwt.exp_seconds == 0 {
        return Err(ConfigError::Invalid("security.jwt.exp_seconds must be > 0"));
    }
    if let Some(s) = cfg.server.ssh.idle_close_seconds {
        if s == 0 {
            return Err(ConfigError::Invalid(
                "server.ssh.idle_close_seconds must be > 0 if set",
            ));
        }
    }
    if cfg.server.ssh.host_key_path.trim().is_empty() {
        return Err(ConfigError::Invalid(
            "server.ssh.host_key_path must not be empty",
        ));
    }
    validate_http_tls(&cfg.server.http)?;
    validate_ssh_tls(&cfg.server.ssh)?;
    cfg.security.http.validate()?;
    Ok(())
}

fn validate_http_tls(http: &HttpConfig) -> Result<(), ConfigError> {
    let tls = &http.tls;
    if tls.enabled {
        let cert = tls
            .cert_path
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::Invalid(
                "server.http.tls.cert_path muss gesetzt sein, wenn TLS aktiviert ist",
            ))?;
        let key = tls
            .key_path
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::Invalid(
                "server.http.tls.key_path muss gesetzt sein, wenn TLS aktiviert ist",
            ))?;
        if cert == key {
            return Err(ConfigError::Invalid(
                "server.http.tls.cert_path und key_path dürfen nicht identisch sein",
            ));
        }
        if tls.cipher_suites.is_empty() {
            return Err(ConfigError::Invalid(
                "server.http.tls.cipher_suites darf bei aktiviertem TLS nicht leer sein",
            ));
        }
        if let Some(interval) = tls.reload_interval_seconds {
            if interval == 0 {
                return Err(ConfigError::Invalid(
                    "server.http.tls.reload_interval_seconds muss > 0 sein",
                ));
            }
        }
    }
    Ok(())
}

fn validate_ssh_tls(ssh: &SshConfig) -> Result<(), ConfigError> {
    if let Some(interval) = ssh.tls.host_key_reload_seconds {
        if interval == 0 {
            return Err(ConfigError::Invalid(
                "server.ssh.tls.host_key_reload_seconds muss > 0 sein",
            ));
        }
    }
    if ssh
        .tls
        .allowed_ciphers
        .iter()
        .any(|cipher| cipher.trim().is_empty())
    {
        return Err(ConfigError::Invalid(
            "server.ssh.tls.allowed_ciphers darf keine leeren Einträge enthalten",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_loads_and_validates() {
        std::env::set_var(
            "FENRIR_DB_POSTGRES_URI",
            "postgresql://localhost:5432/fenrir",
        );
        std::env::set_var("FENRIR_HTTP_TOKEN_ADMIN", "admin-token");
        std::env::set_var("FENRIR_HTTP_TOKEN_OPERATOR", "operator-token");
        std::env::set_var("FENRIR_HTTP_TOKEN_VIEWER", "viewer-token");
        let cfg = load().expect("config should load");
        assert!(!cfg.app.name.is_empty());
    }
}
