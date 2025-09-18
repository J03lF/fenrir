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
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SecuritySection {
    pub kdf: KdfConfig,
    pub jwt: JwtConfig,
    pub allowed_ciphers: Vec<String>,
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

#[derive(Debug, Deserialize, Clone)]
pub struct DbSection {
    pub default_engine: String,
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_loads_and_validates() {
        let cfg = load().expect("config should load");
        assert!(!cfg.app.name.is_empty());
    }
}
