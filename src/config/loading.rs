use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::error::ConfigError;
use super::model::{AppConfig, DbRuntimeMode};
use super::validation::{
    validate_grpc_tls, validate_http_tls, validate_module_registry_tls, validate_ssh_tls,
};

pub(super) const ENV_CONFIG_FILE: &str = "FENRIR_CONFIG_FILE";
pub(super) const ENV_CONFIG_ENV: &str = "FENRIR_CONFIG_ENV";
pub(super) const ENV_ENV: &str = "FENRIR_ENV";
pub(super) const ENV_FILE: &str = "secrets/.env";
pub(super) const ENV_FILE_OVERRIDE: &str = "FENRIR_ENV_FILE";
pub(super) const LOCAL_OVERRIDE_FILE: &str = "config/local.toml";
pub(super) const PROFILE_FILE: &str = "bin/.fenrir-profile";

pub fn load() -> Result<AppConfig, ConfigError> {
    load_env_file_overrides()?;
    let mut builder =
        config::Config::builder().add_source(config::File::from(Path::new("config/default.toml")));

    if let Some(profile) = detect_config_profile()? {
        let profile_path = Path::new("config").join(format!("{}.toml", profile));
        if !profile_path.exists() {
            return Err(ConfigError::MissingConfigFile {
                path: profile_path.display().to_string(),
            });
        }
        builder = builder.add_source(config::File::from(profile_path));
    }

    if let Some(local) = local_override_path() {
        builder = builder.add_source(config::File::from(local));
    }

    if let Some(explicit) = explicit_config_path()? {
        builder = builder.add_source(config::File::from(explicit));
    }

    builder = builder.add_source(config::Environment::with_prefix("FENRIR").separator("__"));

    let mut cfg: AppConfig = builder
        .build()
        .map_err(|e| ConfigError::Anyhow(e.into()))?
        .try_deserialize()
        .map_err(|e| ConfigError::Anyhow(e.into()))?;

    // Apply profile-specific ENV overrides (FENRIR_* without double underscore)
    apply_profile_overrides(&mut cfg);

    validate(&cfg)?;
    Ok(cfg)
}

/// Apply ENV overrides from .fenrir-profile that use simple FENRIR_* names
fn apply_profile_overrides(cfg: &mut AppConfig) {
    // App section overrides
    if let Ok(value) = env::var("FENRIR_DISTRIBUTION") {
        if !value.trim().is_empty() {
            cfg.app.distribution = Some(value);
        }
    }
    if let Ok(value) = env::var("FENRIR_PROFILE") {
        if !value.trim().is_empty() {
            cfg.app.profile = Some(value);
        }
    }
    if let Ok(value) = env::var("FENRIR_DEBUG") {
        cfg.app.debug = matches!(value.to_lowercase().as_str(), "true" | "1" | "yes");
    }

    // SSH user override
    if let Ok(value) = env::var("FENRIR_USER") {
        if !value.trim().is_empty() {
            cfg.server.ssh.user = value;
        }
    }

    // Module paths overrides
    if let Ok(value) = env::var("FENRIR_DEV_MODULE_PATH") {
        if !value.trim().is_empty() {
            cfg.modules.dev_sources.base_path = Some(value);
        }
    }
    if let Ok(value) = env::var("FENRIR_MODULE_INSTALL_DIR") {
        if !value.trim().is_empty() {
            cfg.modules.storage.install_dir = value;
        }
    }
}

pub fn validate(cfg: &AppConfig) -> Result<(), ConfigError> {
    cfg.security.validate()?;
    cfg.telemetry.validate()?;
    cfg.modules.registry.validate()?;
    cfg.modules.storage.validate()?;
    cfg.modules.runtime.validate()?;
    cfg.modules.validate_services()?;
    cfg.audit.validate()?;

    cfg.db
        .runtime
        .validate(cfg.db.default_engine.as_str())?;

    if !matches!(
        cfg.db.default_engine.as_str(),
        "postgres" | "mysql" | "sqlite" | "mongodb"
    ) {
        return Err(ConfigError::Invalid(
            "db.default_engine must be one of postgres|mysql|sqlite|mongodb",
        ));
    }

    let connections = &cfg.db.connections;
    let runtime_mode = cfg.db.runtime.mode;
    let default_settings = match (runtime_mode, cfg.db.default_engine.as_str()) {
        (DbRuntimeMode::External, "postgres") => {
            let settings = connections.postgres.as_ref().ok_or(ConfigError::Invalid(
                "db.connections.postgres.uri must be set when postgres is default",
            ))?;
            settings.resolve_uri("db.connections.postgres.uri")?;
            Some(settings)
        }
        (DbRuntimeMode::External, "mysql") => {
            let settings = connections.mysql.as_ref().ok_or(ConfigError::Invalid(
                "db.connections.mysql.uri must be set when mysql is default",
            ))?;
            settings.resolve_uri("db.connections.mysql.uri")?;
            Some(settings)
        }
        (DbRuntimeMode::External, "sqlite") => {
            let settings = connections.sqlite.as_ref().ok_or(ConfigError::Invalid(
                "db.connections.sqlite.uri must be set when sqlite is default",
            ))?;
            settings.resolve_uri("db.connections.sqlite.uri")?;
            Some(settings)
        }
        (DbRuntimeMode::External, "mongodb") => {
            let settings = connections.mongodb.as_ref().ok_or(ConfigError::Invalid(
                "db.connections.mongodb.uri must be set when mongodb is default",
            ))?;
            settings.resolve_uri("db.connections.mongodb.uri")?;
            Some(settings)
        }
        // Embedded: allow URIs to be optional, but validate if provided for the default engine.
        (DbRuntimeMode::Embedded, "postgres") => {
            if let Some(settings) = connections.postgres.as_ref() {
                settings.resolve_uri("db.connections.postgres.uri")?;
                Some(settings)
            } else {
                None
            }
        }
        (DbRuntimeMode::Embedded, "mysql") => {
            if let Some(settings) = connections.mysql.as_ref() {
                settings.resolve_uri("db.connections.mysql.uri")?;
                Some(settings)
            } else {
                None
            }
        }
        (DbRuntimeMode::Embedded, "sqlite") => {
            if let Some(settings) = connections.sqlite.as_ref() {
                settings.resolve_uri("db.connections.sqlite.uri")?;
                Some(settings)
            } else {
                None
            }
        }
        (DbRuntimeMode::Embedded, "mongodb") => {
            if let Some(settings) = connections.mongodb.as_ref() {
                settings.resolve_uri("db.connections.mongodb.uri")?;
                Some(settings)
            } else {
                None
            }
        }
        _ => unreachable!(),
    };

    if let Some(default_key) = default_settings {
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
    }

    if cfg.server.http.port == 0 || cfg.server.ssh.port == 0 {
        return Err(ConfigError::Invalid("server ports must be > 0"));
    }
    if cfg.server.http.host.trim().is_empty() {
        return Err(ConfigError::Invalid("server.http.host must not be empty"));
    }
    if cfg.server.enable_grpc {
        let grpc = cfg.server.grpc.as_ref().ok_or(ConfigError::Invalid(
            "server.grpc must be configured when server.enable_grpc=true",
        ))?;
        if grpc.host.trim().is_empty() {
            return Err(ConfigError::Invalid("server.grpc.host must not be empty"));
        }
        if grpc.port == 0 {
            return Err(ConfigError::Invalid("server.grpc.port must be > 0"));
        }
        validate_grpc_tls(grpc)?;
    } else if let Some(grpc) = &cfg.server.grpc {
        if grpc.host.trim().is_empty() {
            return Err(ConfigError::Invalid("server.grpc.host must not be empty"));
        }
        if grpc.port == 0 {
            return Err(ConfigError::Invalid("server.grpc.port must be > 0"));
        }
        validate_grpc_tls(grpc)?;
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
    validate_module_registry_tls(&cfg.modules.registry)?;
    if cfg.modules.trust.require_signature {
        let _keyring_path = cfg
            .modules
            .trust
            .keyring_path
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .ok_or(ConfigError::Invalid(
                "modules.trust.keyring_path must be provided when signatures are required",
            ))?;
        if cfg.modules.trust.allowed_signers.is_empty() {
            return Err(ConfigError::Invalid(
                "modules.trust.allowed_signers must list at least one signer when signatures are required",
            ));
        }
        if cfg
            .modules
            .trust
            .allowed_signers
            .iter()
            .any(|signer| signer.trim().is_empty())
        {
            return Err(ConfigError::Invalid(
                "modules.trust.allowed_signers must not contain empty entries",
            ));
        }
    }
    Ok(())
}

fn load_env_file_overrides() -> Result<(), ConfigError> {
    // 1. Load bin/.fenrir-profile first (if exists)
    let profile_path = PathBuf::from(PROFILE_FILE);
    if profile_path.exists() {
        load_env_file(&profile_path)?;
    }

    // 2. Then load secrets/.env or FENRIR_ENV_FILE override
    let override_path = env::var(ENV_FILE_OVERRIDE)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    let candidate = override_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(ENV_FILE));

    if !candidate.exists() {
        if override_path.is_some() {
            return Err(ConfigError::MissingConfigFile {
                path: candidate.display().to_string(),
            });
        }
        return Ok(());
    }

    load_env_file(&candidate)
}

fn load_env_file(path: &Path) -> Result<(), ConfigError> {
    let file = File::open(path).map_err(|err| {
        ConfigError::InvalidMessage(format!("failed to open env file {}: {err}", path.display()))
    })?;
    let reader = BufReader::new(file);
    for (idx, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| {
            ConfigError::InvalidMessage(format!(
                "failed to read env file {} line {}: {err}",
                path.display(),
                idx + 1
            ))
        })?;
        if let Some((key, value)) = parse_env_assignment(line.trim(), idx + 1, path)? {
            if env::var_os(&key).is_some() {
                continue;
            }
            env::set_var(key, value);
        }
    }
    Ok(())
}

fn parse_env_assignment(
    raw: &str,
    line: usize,
    path: &Path,
) -> Result<Option<(String, String)>, ConfigError> {
    if raw.is_empty() || raw.starts_with('#') {
        return Ok(None);
    }

    let trimmed = if let Some(stripped) = raw.strip_prefix("export ") {
        stripped.trim()
    } else {
        raw
    };
    let Some((key_part, value_part)) = trimmed.split_once('=') else {
        return Err(ConfigError::InvalidMessage(format!(
            "env file {} line {} is missing '='",
            path.display(),
            line
        )));
    };
    let key = key_part.trim();
    if key.is_empty() {
        return Err(ConfigError::InvalidMessage(format!(
            "env file {} line {} has empty variable name",
            path.display(),
            line
        )));
    }

    let mut value = value_part.trim().to_string();
    if value.len() >= 2 {
        let first = value.as_bytes().first().copied();
        let last = value.as_bytes().last().copied();
        if let (Some(start), Some(end)) = (first, last) {
            if start == end && (start == b'"' || start == b'\'') {
                value = value[1..value.len() - 1].to_string();
            }
        }
    }
    Ok(Some((key.to_string(), value)))
}

pub(super) fn detect_config_profile() -> Result<Option<String>, ConfigError> {
    for key in [ENV_CONFIG_ENV, ENV_ENV] {
        if let Ok(value) = env::var(key) {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }
            let profile = sanitize_profile(trimmed)?;
            return Ok(Some(profile));
        }
    }
    Ok(None)
}

fn sanitize_profile(raw: &str) -> Result<String, ConfigError> {
    let lower = raw.trim().to_ascii_lowercase();
    if lower
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        Ok(lower)
    } else {
        Err(ConfigError::InvalidProfile {
            value: raw.to_string(),
        })
    }
}

pub(super) fn explicit_config_path() -> Result<Option<PathBuf>, ConfigError> {
    match env::var(ENV_CONFIG_FILE) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(ConfigError::Invalid(
                    "FENRIR_CONFIG_FILE must not be empty when set",
                ));
            }
            let path = PathBuf::from(trimmed);
            if !path.exists() {
                return Err(ConfigError::MissingConfigFile {
                    path: path.display().to_string(),
                });
            }
            Ok(Some(path))
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid(
            "FENRIR_CONFIG_FILE must be valid UTF-8",
        )),
    }
}

fn local_override_path() -> Option<PathBuf> {
    let path = PathBuf::from(LOCAL_OVERRIDE_FILE);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}
