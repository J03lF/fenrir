use serde::Deserialize;
use std::env;
use std::path::{Path, PathBuf};

const ENV_CONFIG_FILE: &str = "FENRIR_CONFIG_FILE";
const ENV_CONFIG_ENV: &str = "FENRIR_CONFIG_ENV";
const ENV_ENV: &str = "FENRIR_ENV";
const LOCAL_OVERRIDE_FILE: &str = "config/local.toml";

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub app: AppSection,
    pub server: ServerSection,
    pub security: SecuritySection,
    pub db: DbSection,
    pub telemetry: TelemetrySection,
    pub audit: AuditSection,
    pub cli: CliSection,
    pub modules: ModulesSection,
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
    #[serde(default)]
    pub grpc: Option<GrpcConfig>,
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

#[derive(Debug, Deserialize, Clone)]
pub struct GrpcConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub tls: GrpcTlsConfig,
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
pub struct GrpcTlsConfig {
    pub enabled: bool,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub client_ca_path: Option<String>,
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
    #[serde(default)]
    pub session: SessionSection,
    #[serde(default)]
    pub identity: IdentitySection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct KdfConfig {
    pub algorithm: String,
    #[serde(default = "default_kdf_version")]
    pub version: u32,
    #[serde(default = "default_argon2_memory_mib")]
    pub memory_mib: u32,
    #[serde(default = "default_argon2_iterations")]
    pub iterations: u32,
    #[serde(default = "default_argon2_parallelism")]
    pub parallelism: u32,
    #[serde(default = "default_kdf_salt_len")]
    pub salt_length: u32,
    #[serde(default = "default_kdf_output_len")]
    pub output_length: u32,
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

#[derive(Debug, Deserialize, Clone)]
pub struct IdentitySection {
    #[serde(default = "default_identity_provider")]
    pub provider: IdentityProviderKind,
    #[serde(default = "default_identity_environment")]
    pub environment: String,
    #[serde(default = "default_identity_instance_id")]
    pub instance_id: String,
    #[serde(default)]
    pub embedded: IdentityEmbeddedSection,
    #[serde(default)]
    pub external: IdentityExternalSection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct IdentityEmbeddedSection {
    #[serde(default = "default_identity_store_path")]
    pub store_path: String,
    #[serde(default = "default_identity_audience")]
    pub audience: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct IdentityExternalSection {
    pub base_url: Option<String>,
    pub jwks_url: Option<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default = "default_jwks_refresh_seconds")]
    pub jwks_refresh_seconds: u64,
    #[serde(default = "default_identity_audience")]
    pub audience: String,
    #[serde(default)]
    pub tls: IdentityExternalTlsSection,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct IdentityExternalTlsSection {
    #[serde(default)]
    pub ca_cert_path: Option<String>,
    #[serde(default)]
    pub client_cert_path: Option<String>,
    #[serde(default)]
    pub client_key_path: Option<String>,
    #[serde(default)]
    pub accept_invalid_certs: bool,
}

#[derive(Debug, Clone, Default)]
pub struct IdentityExternalTlsResolved {
    pub ca_cert_path: Option<String>,
    pub client_cert_path: Option<String>,
    pub client_key_path: Option<String>,
    pub accept_invalid_certs: bool,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IdentityProviderKind {
    Embedded,
    External,
}

impl Default for IdentitySection {
    fn default() -> Self {
        Self {
            provider: default_identity_provider(),
            environment: default_identity_environment(),
            instance_id: default_identity_instance_id(),
            embedded: IdentityEmbeddedSection::default(),
            external: IdentityExternalSection::default(),
        }
    }
}

impl Default for IdentityEmbeddedSection {
    fn default() -> Self {
        Self {
            store_path: default_identity_store_path(),
            audience: default_identity_audience(),
        }
    }
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
    pub tracing: TelemetryTracingSection,
    #[serde(default)]
    pub metrics: TelemetryMetricsSection,
    #[serde(default)]
    pub health: TelemetryHealthSection,
    #[serde(default)]
    pub system: TelemetrySystemSection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelemetryTracingSection {
    pub level: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelemetryMetricsSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub exporter: Option<String>,
}

impl Default for TelemetryMetricsSection {
    fn default() -> Self {
        Self {
            enabled: true,
            exporter: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelemetryHealthSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for TelemetryHealthSection {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelemetrySystemSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub interval_ms: Option<u64>,
}

impl Default for TelemetrySystemSection {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_ms: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuditSection {
    pub enabled: bool,
    #[serde(default)]
    pub buffer_capacity: Option<usize>,
    #[serde(default)]
    pub storage: AuditStorageSection,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct AuditStorageSection {
    pub path: Option<String>,
    #[serde(default)]
    pub retention_hours: Option<u64>,
    #[serde(default)]
    pub persist_interval_seconds: Option<u64>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CliSection {
    pub prompt_theme: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModulesSection {
    pub registry: ModuleRegistrySection,
    pub storage: ModuleStorageSection,
    #[serde(default)]
    pub runtime: ModuleRuntimeSection,
    #[serde(default)]
    pub bootstrap: Vec<String>,
    #[serde(default)]
    pub trust: ModuleTrustSection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModuleRegistrySection {
    pub url: String,
    #[serde(default)]
    pub allow_offline: bool,
    #[serde(default)]
    pub offline_dirs: Vec<String>,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default)]
    pub tls: ModuleRegistryTlsSection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModuleRuntimeSection {
    #[serde(default = "default_module_runtime_engine")]
    pub engine: ModuleRuntimeEngine,
}

impl Default for ModuleRuntimeSection {
    fn default() -> Self {
        Self {
            engine: default_module_runtime_engine(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModuleRuntimeEngine {
    Process,
    Stub,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModuleStorageSection {
    pub install_dir: String,
    #[serde(default)]
    pub cache_dir: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ModuleRegistryTlsSection {
    pub ca_cert_path: Option<String>,
    pub client_cert_path: Option<String>,
    pub client_key_path: Option<String>,
    #[serde(default)]
    pub accept_invalid_certs: bool,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ModuleTrustSection {
    #[serde(default = "default_require_signature")]
    pub require_signature: bool,
    #[serde(default)]
    pub allowed_signers: Vec<String>,
    #[serde(default)]
    pub keyring_path: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct SessionSection {
    #[serde(default = "default_session_lifetime_seconds")]
    pub lifetime_seconds: u64,
    #[serde(default = "default_session_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_session_cleanup_interval_seconds")]
    pub cleanup_interval_seconds: u64,
}

fn default_require_signature() -> bool {
    true
}

fn default_true() -> bool {
    true
}

fn default_module_runtime_engine() -> ModuleRuntimeEngine {
    ModuleRuntimeEngine::Process
}

fn default_kdf_version() -> u32 {
    1
}

fn default_argon2_memory_mib() -> u32 {
    64
}

fn default_argon2_iterations() -> u32 {
    3
}

fn default_argon2_parallelism() -> u32 {
    2
}

fn default_kdf_salt_len() -> u32 {
    16
}

fn default_kdf_output_len() -> u32 {
    32
}

fn default_session_lifetime_seconds() -> u64 {
    3600
}

fn default_session_idle_timeout_seconds() -> u64 {
    900
}

fn default_session_cleanup_interval_seconds() -> u64 {
    300
}

fn default_identity_provider() -> IdentityProviderKind {
    IdentityProviderKind::Embedded
}

fn default_identity_environment() -> String {
    "dev".to_string()
}

fn default_identity_instance_id() -> String {
    "local".to_string()
}

fn default_identity_store_path() -> String {
    "runtime/identity/store.json".to_string()
}

fn default_identity_audience() -> String {
    "fenrir-control-plane".to_string()
}

fn default_jwks_refresh_seconds() -> u64 {
    300
}

#[derive(thiserror::Error, Debug)]
pub enum ConfigError {
    #[error("{0}")]
    Anyhow(#[from] anyhow::Error),
    #[error("invalid configuration: {0}")]
    Invalid(&'static str),
    #[error("missing environment variable {var} for {key}")]
    MissingEnv { key: &'static str, var: String },
    #[error("config profile '{value}' contains invalid characters (allowed: a-z, 0-9, '-', '_')")]
    InvalidProfile { value: String },
    #[error("config file not found: {path}")]
    MissingConfigFile { path: String },
}

impl HttpSecuritySection {
    pub fn resolve_control_tokens(&self) -> Result<Vec<HttpControlToken>, ConfigError> {
        let mut resolved = Vec::with_capacity(self.control_tokens.len());
        for entry in &self.control_tokens {
            let (role, value) = parse_role_token(entry)?;
            let secret = resolve_secret(value, "security.http.control_tokens")?;
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

impl KdfConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.algorithm.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "security.kdf.algorithm must not be empty",
            ));
        }
        match self.algorithm.to_ascii_lowercase().as_str() {
            "argon2id" | "argon2" => {}
            other => {
                return Err(ConfigError::Invalid(match other {
                    _ => "security.kdf.algorithm must be argon2id",
                }))
            }
        }
        if self.version == 0 {
            return Err(ConfigError::Invalid("security.kdf.version must be > 0"));
        }
        if self.memory_mib == 0 {
            return Err(ConfigError::Invalid("security.kdf.memory_mib must be > 0"));
        }
        if self.iterations == 0 {
            return Err(ConfigError::Invalid("security.kdf.iterations must be > 0"));
        }
        if self.parallelism == 0 {
            return Err(ConfigError::Invalid("security.kdf.parallelism must be > 0"));
        }
        if self.salt_length < 12 {
            return Err(ConfigError::Invalid(
                "security.kdf.salt_length must be >= 12 bytes",
            ));
        }
        if self.output_length < 16 {
            return Err(ConfigError::Invalid(
                "security.kdf.output_length must be >= 16 bytes",
            ));
        }
        Ok(())
    }
}

impl SecuritySection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.allowed_ciphers.is_empty() {
            return Err(ConfigError::Invalid(
                "security.allowed_ciphers must contain at least one cipher",
            ));
        }

        for cipher in &self.allowed_ciphers {
            match cipher.trim().to_ascii_lowercase().as_str() {
                "aes-gcm" | "aes256-gcm" | "aes_256_gcm" | "xchacha20-poly1305" => {}
                _ => {
                    return Err(ConfigError::Invalid(
                        "security.allowed_ciphers contains unsupported cipher",
                    ))
                }
            }
        }

        self.kdf.validate()?;
        self.http.validate()?;
        self.session.validate()?;
        self.identity.validate()?;
        Ok(())
    }
}

impl IdentitySection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.environment.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "security.identity.environment must not be empty",
            ));
        }
        if self.instance_id.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "security.identity.instance_id must not be empty",
            ));
        }
        match self.provider {
            IdentityProviderKind::Embedded => {
                if self.embedded.store_path.trim().is_empty() {
                    return Err(ConfigError::Invalid(
                        "security.identity.embedded.store_path must not be empty",
                    ));
                }
                if self.embedded.audience.trim().is_empty() {
                    return Err(ConfigError::Invalid(
                        "security.identity.embedded.audience must not be empty",
                    ));
                }
            }
            IdentityProviderKind::External => {
                let base = self.external.base_url.as_ref().ok_or_else(|| {
                    ConfigError::Invalid(
                        "security.identity.external.base_url must be set for external provider",
                    )
                })?;
                if base.trim().is_empty() {
                    return Err(ConfigError::Invalid(
                        "security.identity.external.base_url must not be empty",
                    ));
                }
                let env_lower = self.environment.trim().to_ascii_lowercase();
                validate_identity_tls(&self.external.tls, env_lower.as_str())?;
                if let Some(jwks) = &self.external.jwks_url {
                    if jwks.trim().is_empty() {
                        return Err(ConfigError::Invalid(
                            "security.identity.external.jwks_url must not be empty when set",
                        ));
                    }
                    if matches!(env_lower.as_str(), "prod" | "production")
                        && !jwks.trim().to_ascii_lowercase().starts_with("https://")
                    {
                        return Err(ConfigError::Invalid(
                            "security.identity.external.jwks_url must use https:// in production",
                        ));
                    }
                }
                if self.external.jwks_refresh_seconds == 0 {
                    return Err(ConfigError::Invalid(
                        "security.identity.external.jwks_refresh_seconds must be > 0",
                    ));
                }
                if self.external.audience.trim().is_empty() {
                    return Err(ConfigError::Invalid(
                        "security.identity.external.audience must not be empty",
                    ));
                }
                if matches!(env_lower.as_str(), "prod" | "production") {
                    if !base.trim().to_ascii_lowercase().starts_with("https://") {
                        return Err(ConfigError::Invalid(
                            "security.identity.external.base_url must use https:// in production",
                        ));
                    }
                    if self
                        .external
                        .auth_token
                        .as_ref()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .is_none()
                    {
                        return Err(ConfigError::Invalid(
                            "security.identity.external.auth_token must be configured in production",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn audience(&self) -> &str {
        match self.provider {
            IdentityProviderKind::Embedded => self.embedded.audience.as_str(),
            IdentityProviderKind::External => self.external.audience.as_str(),
        }
    }

    pub fn store_path(&self) -> PathBuf {
        PathBuf::from(self.embedded.store_path.clone())
    }

    pub fn resolve_external_auth_token(&self) -> Result<Option<String>, ConfigError> {
        resolve_optional_secret(
            &self.external.auth_token,
            "security.identity.external.auth_token",
        )
    }

    pub fn resolve_external_tls(&self) -> Result<IdentityExternalTlsResolved, ConfigError> {
        self.external.resolve_tls()
    }
}

impl IdentityExternalSection {
    pub fn resolve_tls(&self) -> Result<IdentityExternalTlsResolved, ConfigError> {
        Ok(IdentityExternalTlsResolved {
            ca_cert_path: resolve_optional_secret(
                &self.tls.ca_cert_path,
                "security.identity.external.tls.ca_cert_path",
            )?,
            client_cert_path: resolve_optional_secret(
                &self.tls.client_cert_path,
                "security.identity.external.tls.client_cert_path",
            )?,
            client_key_path: resolve_optional_secret(
                &self.tls.client_key_path,
                "security.identity.external.tls.client_key_path",
            )?,
            accept_invalid_certs: self.tls.accept_invalid_certs,
        })
    }
}

impl ModuleRegistrySection {
    pub fn resolved_auth_token(&self) -> Result<Option<String>, ConfigError> {
        resolve_optional_secret(&self.auth_token, "modules.registry.auth_token")
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.url.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "modules.registry.url must not be empty",
            ));
        }
        if let Some(token) = self.resolved_auth_token()? {
            if token.len() < 16 {
                return Err(ConfigError::Invalid(
                    "modules.registry.auth_token must be at least 16 characters",
                ));
            }
        }
        for dir in &self.offline_dirs {
            if dir.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "modules.registry.offline_dirs must not contain empty paths",
                ));
            }
        }
        Ok(())
    }
}

impl ModuleStorageSection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.install_dir.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "modules.storage.install_dir must not be empty",
            ));
        }
        if let Some(cache) = &self.cache_dir {
            if cache.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "modules.storage.cache_dir must not be empty when set",
                ));
            }
        }
        Ok(())
    }
}

impl ModuleRuntimeSection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        match self.engine {
            ModuleRuntimeEngine::Process | ModuleRuntimeEngine::Stub => Ok(()),
        }
    }
}

impl SessionSection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.lifetime_seconds == 0 {
            return Err(ConfigError::Invalid(
                "security.session.lifetime_seconds must be > 0",
            ));
        }
        if self.idle_timeout_seconds == 0 {
            return Err(ConfigError::Invalid(
                "security.session.idle_timeout_seconds must be > 0",
            ));
        }
        if self.idle_timeout_seconds > self.lifetime_seconds {
            return Err(ConfigError::Invalid(
                "security.session.idle_timeout_seconds must not exceed lifetime_seconds",
            ));
        }
        if self.cleanup_interval_seconds == 0 {
            return Err(ConfigError::Invalid(
                "security.session.cleanup_interval_seconds must be > 0",
            ));
        }
        if self.cleanup_interval_seconds > self.lifetime_seconds {
            return Err(ConfigError::Invalid(
                "security.session.cleanup_interval_seconds must not exceed lifetime_seconds",
            ));
        }
        Ok(())
    }
}

impl AuditSection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if let Some(capacity) = self.buffer_capacity {
            if capacity == 0 {
                return Err(ConfigError::Invalid("audit.buffer_capacity must be > 0"));
            }
        }
        self.storage.validate(self.enabled)
    }

    pub fn buffer_capacity(&self) -> usize {
        self.buffer_capacity.unwrap_or(1024)
    }
}

impl AuditStorageSection {
    fn validate(&self, audit_enabled: bool) -> Result<(), ConfigError> {
        if let Some(hours) = self.retention_hours {
            if hours == 0 {
                return Err(ConfigError::Invalid(
                    "audit.storage.retention_hours must be > 0 when set",
                ));
            }
        }
        if let Some(interval) = self.persist_interval_seconds {
            if interval == 0 {
                return Err(ConfigError::Invalid(
                    "audit.storage.persist_interval_seconds must be > 0 when set",
                ));
            }
        }

        match self
            .path
            .as_ref()
            .map(|path| path.trim())
            .filter(|p| !p.is_empty())
        {
            Some(_) => Ok(()),
            None if audit_enabled => Err(ConfigError::Invalid(
                "audit.storage.path must be set when audit.enabled=true",
            )),
            None => Ok(()),
        }
    }

    pub fn path(&self) -> Option<&str> {
        self.path
            .as_deref()
            .map(|path| path.trim())
            .filter(|p| !p.is_empty())
    }

    pub fn retention_hours(&self) -> Option<u64> {
        self.retention_hours
    }

    pub fn persist_interval_seconds(&self) -> Option<u64> {
        self.persist_interval_seconds
    }
}

impl TelemetrySection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.tracing.level.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "telemetry.tracing.level must not be empty",
            ));
        }
        if let Some(exporter) = self.metrics.exporter.as_ref() {
            if exporter.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "telemetry.metrics.exporter must not be empty when set",
                ));
            }
        }
        if let Some(interval) = self.system.interval_ms {
            if interval == 0 {
                return Err(ConfigError::Invalid(
                    "telemetry.system.interval_ms must be > 0",
                ));
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

fn resolve_secret(value: &str, key: &'static str) -> Result<String, ConfigError> {
    if let Some(env) = value.strip_prefix("env:") {
        let var = env.trim();
        match std::env::var(var) {
            Ok(secret) if !secret.trim().is_empty() => Ok(secret),
            Ok(_) => Err(ConfigError::Invalid(
                "environment variable referenced in configuration must not be empty",
            )),
            Err(_) => Err(ConfigError::MissingEnv {
                key,
                var: var.to_string(),
            }),
        }
    } else {
        Ok(value.to_string())
    }
}

fn resolve_optional_secret(
    value: &Option<String>,
    key: &'static str,
) -> Result<Option<String>, ConfigError> {
    match value {
        Some(raw) => {
            let secret = resolve_secret(raw, key)?;
            if secret.trim().is_empty() {
                return Err(ConfigError::Invalid("secret values must not be empty"));
            }
            Ok(Some(secret))
        }
        None => Ok(None),
    }
}

fn detect_config_profile() -> Result<Option<String>, ConfigError> {
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

fn explicit_config_path() -> Result<Option<PathBuf>, ConfigError> {
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

pub fn load() -> Result<AppConfig, ConfigError> {
    let mut builder =
        config::Config::builder().add_source(config::File::from(Path::new("config/prod.toml")));

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
    cfg.security.validate()?;
    cfg.telemetry.validate()?;
    cfg.modules.registry.validate()?;
    cfg.modules.storage.validate()?;
    cfg.modules.runtime.validate()?;
    cfg.audit.validate()?;

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

fn validate_grpc_tls(grpc: &GrpcConfig) -> Result<(), ConfigError> {
    let tls = &grpc.tls;
    if tls.enabled {
        let cert = tls
            .cert_path
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::Invalid(
                "server.grpc.tls.cert_path must be set when TLS is enabled",
            ))?;
        let key = tls
            .key_path
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or(ConfigError::Invalid(
                "server.grpc.tls.key_path must be set when TLS is enabled",
            ))?;
        if cert == key {
            return Err(ConfigError::Invalid(
                "server.grpc.tls.cert_path and key_path must differ",
            ));
        }
        if tls.cipher_suites.is_empty() {
            return Err(ConfigError::Invalid(
                "server.grpc.tls.cipher_suites must not be empty when TLS is enabled",
            ));
        }
        if tls
            .cipher_suites
            .iter()
            .any(|suite| suite.trim().is_empty())
        {
            return Err(ConfigError::Invalid(
                "server.grpc.tls.cipher_suites must not contain empty entries",
            ));
        }
        if let Some(ca) = tls.client_ca_path.as_ref() {
            if ca.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "server.grpc.tls.client_ca_path must not be empty when set",
                ));
            }
        }
        if let Some(interval) = tls.reload_interval_seconds {
            if interval == 0 {
                return Err(ConfigError::Invalid(
                    "server.grpc.tls.reload_interval_seconds must be > 0",
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

fn validate_identity_tls(
    tls: &IdentityExternalTlsSection,
    environment: &str,
) -> Result<(), ConfigError> {
    if let Some(ca) = tls.ca_cert_path.as_ref() {
        if ca.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "security.identity.external.tls.ca_cert_path must not be empty when set",
            ));
        }
    }

    match (
        tls.client_cert_path.as_ref().map(|s| s.trim()),
        tls.client_key_path.as_ref().map(|s| s.trim()),
    ) {
        (Some(cert), Some(key)) => {
            if cert.is_empty() || key.is_empty() {
                return Err(ConfigError::Invalid(
                    "security.identity.external.tls.client_cert_path and client_key_path must not be empty",
                ));
            }
        }
        (None, None) => {}
        _ => {
            return Err(ConfigError::Invalid(
                "security.identity.external.tls.client_cert_path and client_key_path must be provided together",
            ));
        }
    }

    if matches!(environment, "prod" | "production") && tls.accept_invalid_certs {
        return Err(ConfigError::Invalid(
            "security.identity.external.tls.accept_invalid_certs must be false in production",
        ));
    }

    Ok(())
}

fn validate_module_registry_tls(cfg: &ModuleRegistrySection) -> Result<(), ConfigError> {
    let tls = &cfg.tls;

    if let Some(ca) = tls.ca_cert_path.as_ref() {
        if ca.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "modules.registry.tls.ca_cert_path darf nicht leer sein",
            ));
        }
    }

    match (tls.client_cert_path.as_ref(), tls.client_key_path.as_ref()) {
        (Some(cert), Some(key)) => {
            if cert.trim().is_empty() || key.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "modules.registry.tls.client_cert_path und client_key_path dürfen nicht leer sein",
                ));
            }
        }
        (None, None) => {}
        _ => {
            return Err(ConfigError::Invalid(
                "modules.registry.tls.client_cert_path und client_key_path müssen gemeinsam gesetzt werden",
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn test_mutex() -> &'static Mutex<()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn default_config_loads_and_validates() {
        let _lock = test_mutex().lock().unwrap();
        env::set_var(
            "FENRIR_DB_POSTGRES_URI",
            "postgresql://localhost:5432/fenrir",
        );
        env::set_var("FENRIR_HTTP_TOKEN_ADMIN", "admin-token-example");
        env::set_var("FENRIR_HTTP_TOKEN_OPERATOR", "operator-token-example");
        env::set_var("FENRIR_HTTP_TOKEN_VIEWER", "viewer-token-example");
        env::set_var("FENRIR_REGISTRY_TOKEN", "registry-token-example-123");
        env::remove_var(ENV_CONFIG_FILE);
        env::remove_var(ENV_CONFIG_ENV);
        env::remove_var(ENV_ENV);

        let cfg = load().expect("config should load");
        assert!(!cfg.app.name.is_empty());
        env::remove_var("FENRIR_DB_POSTGRES_URI");
        env::remove_var("FENRIR_HTTP_TOKEN_ADMIN");
        env::remove_var("FENRIR_HTTP_TOKEN_OPERATOR");
        env::remove_var("FENRIR_HTTP_TOKEN_VIEWER");
        env::remove_var("FENRIR_REGISTRY_TOKEN");
    }

    #[test]
    fn config_env_requires_existing_profile_file() {
        let _lock = test_mutex().lock().unwrap();
        env::set_var(
            "FENRIR_DB_POSTGRES_URI",
            "postgresql://localhost:5432/fenrir",
        );
        env::set_var("FENRIR_HTTP_TOKEN_ADMIN", "admin-token-example");
        env::set_var("FENRIR_HTTP_TOKEN_OPERATOR", "operator-token-example");
        env::set_var("FENRIR_HTTP_TOKEN_VIEWER", "viewer-token-example");
        env::set_var("FENRIR_REGISTRY_TOKEN", "registry-token-example-123");
        env::set_var(ENV_CONFIG_ENV, "does-not-exist");
        env::remove_var(ENV_CONFIG_FILE);
        env::remove_var(ENV_ENV);

        let err = load().expect_err("profile should be missing");
        match err {
            ConfigError::MissingConfigFile { path } => {
                assert!(path.ends_with("config/does-not-exist.toml"));
            }
            other => panic!("expected MissingConfigFile, got {other:?}"),
        }
        env::remove_var("FENRIR_DB_POSTGRES_URI");
        env::remove_var("FENRIR_HTTP_TOKEN_ADMIN");
        env::remove_var("FENRIR_HTTP_TOKEN_OPERATOR");
        env::remove_var("FENRIR_HTTP_TOKEN_VIEWER");
        env::remove_var("FENRIR_REGISTRY_TOKEN");
        env::remove_var(ENV_CONFIG_ENV);
    }

    #[test]
    fn explicit_config_file_must_exist() {
        let _lock = test_mutex().lock().unwrap();
        let missing = env::temp_dir().join("fenrir-test-missing-config.toml");
        if missing.exists() {
            std::fs::remove_file(&missing).ok();
        }
        env::set_var(ENV_CONFIG_FILE, missing.to_string_lossy().to_string());
        env::remove_var(ENV_CONFIG_ENV);
        env::remove_var(ENV_ENV);

        let err = explicit_config_path().expect_err("should fail for missing file");
        match err {
            ConfigError::MissingConfigFile { .. } => {}
            other => panic!("expected missing config file error, got {other:?}"),
        }
        env::remove_var(ENV_CONFIG_FILE);
    }
}
