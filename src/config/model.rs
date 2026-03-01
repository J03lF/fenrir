use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    str::FromStr,
};

use super::{error::ConfigError, validation::validate_identity_tls};
use crate::security::auth::Role;

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
    #[serde(default)]
    pub runtime: RuntimeSection,
}

/// Runtime paths configuration
#[derive(Debug, Deserialize, Clone)]
pub struct RuntimeSection {
    /// Base directory for all runtime data (identity, db, migrations, etc.)
    /// Can be absolute or relative to the working directory.
    /// Defaults to "runtime" or FENRIR_RUNTIME_DIR env var.
    #[serde(default = "default_runtime_base_path")]
    pub base_path: String,

    /// Directory for identity store (relative to base_path if not absolute)
    #[serde(default = "default_runtime_identity_dir")]
    pub identity_dir: String,

    /// Directory for database files (relative to base_path if not absolute)
    #[serde(default = "default_runtime_db_dir")]
    pub db_dir: String,

    /// Directory for migration state (relative to base_path if not absolute)
    #[serde(default = "default_runtime_migrations_dir")]
    pub migrations_dir: String,

    /// Directory for IPC sockets (relative to base_path if not absolute)
    #[serde(default = "default_runtime_ipc_dir")]
    pub ipc_dir: String,

    /// Directory for scheduler state (relative to base_path if not absolute)
    #[serde(default = "default_runtime_scheduler_dir")]
    pub scheduler_dir: String,
}

impl Default for RuntimeSection {
    fn default() -> Self {
        Self {
            base_path: default_runtime_base_path(),
            identity_dir: default_runtime_identity_dir(),
            db_dir: default_runtime_db_dir(),
            migrations_dir: default_runtime_migrations_dir(),
            ipc_dir: default_runtime_ipc_dir(),
            scheduler_dir: default_runtime_scheduler_dir(),
        }
    }
}

impl RuntimeSection {
    /// Resolve the base path, considering FENRIR_RUNTIME_DIR env var
    pub fn resolve_base_path(&self) -> std::path::PathBuf {
        // Env var takes precedence
        if let Ok(env_dir) = std::env::var("FENRIR_RUNTIME_DIR") {
            return std::path::PathBuf::from(env_dir);
        }
        std::path::PathBuf::from(&self.base_path)
    }

    /// Resolve a subdirectory path (absolute or relative to base_path)
    pub fn resolve_path(&self, subdir: &str) -> std::path::PathBuf {
        let base = self.resolve_base_path();
        let path = std::path::PathBuf::from(subdir);
        if path.is_absolute() {
            path
        } else {
            base.join(path)
        }
    }

    /// Get resolved identity directory
    pub fn identity_path(&self) -> std::path::PathBuf {
        self.resolve_path(&self.identity_dir)
    }

    /// Get resolved database directory
    pub fn db_path(&self) -> std::path::PathBuf {
        self.resolve_path(&self.db_dir)
    }

    /// Get resolved migrations directory
    pub fn migrations_path(&self) -> std::path::PathBuf {
        self.resolve_path(&self.migrations_dir)
    }

    /// Get resolved IPC directory
    pub fn ipc_path(&self) -> std::path::PathBuf {
        self.resolve_path(&self.ipc_dir)
    }

    /// Get resolved scheduler directory
    pub fn scheduler_path(&self) -> std::path::PathBuf {
        self.resolve_path(&self.scheduler_dir)
    }
}

fn default_runtime_base_path() -> String {
    "runtime".to_string()
}

fn default_runtime_identity_dir() -> String {
    "identity".to_string()
}

fn default_runtime_db_dir() -> String {
    "db".to_string()
}

fn default_runtime_migrations_dir() -> String {
    "migrations".to_string()
}

fn default_runtime_ipc_dir() -> String {
    "ipc".to_string()
}

fn default_runtime_scheduler_dir() -> String {
    "scheduler".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppSection {
    pub name: String,
    pub version: String,
    /// Distribution identifier (e.g. "1.4.0") - can be set via FENRIR_DISTRIBUTION
    #[serde(default)]
    pub distribution: Option<String>,
    /// Profile name (e.g. "LOCAL_DEV", "STAGING") - can be set via FENRIR_PROFILE  
    #[serde(default)]
    pub profile: Option<String>,
    /// Debug mode flag - can be set via FENRIR_DEBUG
    #[serde(default)]
    pub debug: bool,
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
    pub service_tokens: ServiceTokenSection,
    #[serde(default)]
    pub identity: IdentitySection,
    #[serde(default)]
    pub password_policy: PasswordPolicyConfig,
}

/// Password policy configuration for first-time setup
#[derive(Debug, Deserialize, Clone)]
pub struct PasswordPolicyConfig {
    /// Minimum password length (default: 12 for prod, 4 for dev)
    #[serde(default = "default_password_min_length")]
    pub min_length: usize,
    /// Maximum password length (default: 128)
    #[serde(default = "default_password_max_length")]
    pub max_length: usize,
    /// Require at least one uppercase letter
    #[serde(default)]
    pub require_uppercase: bool,
    /// Require at least one lowercase letter
    #[serde(default)]
    pub require_lowercase: bool,
    /// Require at least one digit
    #[serde(default)]
    pub require_digit: bool,
    /// Require at least one special character
    #[serde(default)]
    pub require_special: bool,
    /// Check against common password list
    #[serde(default = "default_true")]
    pub check_common_passwords: bool,
}

impl Default for PasswordPolicyConfig {
    fn default() -> Self {
        Self {
            min_length: default_password_min_length(),
            max_length: default_password_max_length(),
            require_uppercase: false,
            require_lowercase: false,
            require_digit: false,
            require_special: false,
            check_common_passwords: true,
        }
    }
}

fn default_password_min_length() -> usize {
    4
}

fn default_password_max_length() -> usize {
    128
}

fn default_true() -> bool {
    true
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

/// Storage backend for embedded identity provider
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IdentityStorageKind {
    /// Store identity data in JSON file (default for dev)
    #[default]
    File,
    /// Store identity data in database
    Db,
}

#[derive(Debug, Deserialize, Clone)]
pub struct IdentityEmbeddedSection {
    /// Storage backend: "file" (default) or "db"
    #[serde(default)]
    pub storage: IdentityStorageKind,
    /// Path to JSON file store (only used when storage = "file")
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

impl IdentityProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            IdentityProviderKind::Embedded => "Embedded",
            IdentityProviderKind::External => "External",
        }
    }
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
            storage: IdentityStorageKind::default(),
            store_path: default_identity_store_path(),
            audience: default_identity_audience(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpControlToken {
    pub role: Role,
    pub secret: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DbSection {
    pub default_engine: String,
    #[serde(default)]
    pub connections: DbConnections,
    #[serde(default)]
    pub runtime: DbRuntimeSection,
    #[serde(default)]
    pub schema: DbSchemaSection,
    #[serde(default)]
    pub backup: DbBackupSection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DbSchemaSection {
    /// Directory for exported StarUML schemas
    #[serde(default = "default_schema_export_dir")]
    pub export_dir: String,
    /// Directory for importing StarUML schemas
    #[serde(default = "default_schema_import_dir")]
    pub import_dir: String,
    /// Create backup before import
    #[serde(default = "default_true", alias = "auto_backup")]
    pub backup_before_import: bool,
}

impl Default for DbSchemaSection {
    fn default() -> Self {
        Self {
            export_dir: default_schema_export_dir(),
            import_dir: default_schema_import_dir(),
            backup_before_import: true,
        }
    }
}

fn default_schema_export_dir() -> String {
    "runtime/schemas".to_string()
}

fn default_schema_import_dir() -> String {
    "runtime/schemas".to_string()
}

/// Database backup configuration
#[derive(Debug, Deserialize, Clone)]
pub struct DbBackupSection {
    /// Enable automatic backups
    #[serde(default)]
    pub enabled: bool,
    /// Backup schedule: "hourly", "daily", "weekly", or cron expression
    #[serde(default = "default_backup_schedule")]
    pub schedule: String,
    /// Number of backups to retain (oldest are deleted)
    #[serde(default = "default_backup_retention")]
    pub retention_count: usize,
    /// Backup directory (relative to runtime base or absolute)
    #[serde(default = "default_backup_path")]
    pub path: String,
    /// Minimum free disk space in MB required before backup
    #[serde(default = "default_backup_min_disk_mb")]
    pub min_disk_space_mb: u64,
    /// Skip backup if DB is not healthy
    #[serde(default = "default_true")]
    pub require_healthy: bool,
    /// Verify backup integrity after creation
    #[serde(default = "default_true")]
    pub verify_integrity: bool,
    /// Maximum allowed size deviation from last backup (percentage, 0 = disabled)
    #[serde(default)]
    pub anomaly_threshold_pct: u32,
    /// Path to pg_basebackup binary (for Postgres backups)
    #[serde(default)]
    pub pg_basebackup_path: Option<String>,
}

impl Default for DbBackupSection {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: default_backup_schedule(),
            retention_count: default_backup_retention(),
            path: default_backup_path(),
            min_disk_space_mb: default_backup_min_disk_mb(),
            require_healthy: true,
            verify_integrity: true,
            anomaly_threshold_pct: 0,
            pg_basebackup_path: None,
        }
    }
}

fn default_backup_schedule() -> String {
    "daily".to_string()
}

fn default_backup_retention() -> usize {
    7
}

fn default_backup_path() -> String {
    "backups".to_string()
}

fn default_backup_min_disk_mb() -> u64 {
    500 // 500 MB minimum
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

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DbRuntimeMode {
    External,
    Embedded,
}

impl Default for DbRuntimeMode {
    fn default() -> Self {
        Self::External
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddedEngineKind {
    Sqlite,
    Postgres,
}

impl EmbeddedEngineKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EmbeddedEngineKind::Sqlite => "sqlite",
            EmbeddedEngineKind::Postgres => "postgres",
        }
    }
}

impl Default for EmbeddedEngineKind {
    fn default() -> Self {
        Self::Sqlite
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct DbRuntimeSection {
    #[serde(default)]
    pub mode: DbRuntimeMode,
    #[serde(default)]
    pub embedded: DbEmbeddedSection,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DbEmbeddedSection {
    #[serde(default)]
    pub engine: EmbeddedEngineKind,
    #[serde(default)]
    pub sqlite: DbEmbeddedSqlite,
    #[serde(default)]
    pub postgres: DbEmbeddedPostgres,
    #[serde(default)]
    pub security: DbEmbeddedSecuritySection,
}

impl Default for DbEmbeddedSection {
    fn default() -> Self {
        Self {
            engine: EmbeddedEngineKind::Sqlite,
            sqlite: DbEmbeddedSqlite::default(),
            postgres: DbEmbeddedPostgres::default(),
            security: DbEmbeddedSecuritySection::default(),
        }
    }
}

/// Security settings for embedded database connections
#[derive(Debug, Deserialize, Clone)]
pub struct DbEmbeddedSecuritySection {
    /// Authentication method: "trust" (legacy, insecure) or "scram-sha-256" (secure)
    #[serde(default = "default_db_auth_method")]
    pub auth_method: DbAuthMethod,
    /// Prefer Unix socket over TCP (more secure, no network)
    #[serde(default = "default_true")]
    pub prefer_unix_socket: bool,
    /// Credential rotation interval in hours (0 = disabled)
    #[serde(default)]
    pub credential_rotation_hours: u32,
}

impl Default for DbEmbeddedSecuritySection {
    fn default() -> Self {
        Self {
            auth_method: default_db_auth_method(),
            prefer_unix_socket: true,
            credential_rotation_hours: 0,
        }
    }
}

/// Database authentication method
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DbAuthMethod {
    /// Trust authentication (no password) - insecure, legacy only
    Trust,
    /// SCRAM-SHA-256 password authentication (secure)
    #[serde(rename = "scram-sha-256")]
    ScramSha256,
}

impl DbAuthMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            DbAuthMethod::Trust => "trust",
            DbAuthMethod::ScramSha256 => "scram-sha-256",
        }
    }

    pub fn as_pg_arg(&self) -> &'static str {
        self.as_str()
    }

    pub fn requires_password(&self) -> bool {
        match self {
            DbAuthMethod::Trust => false,
            DbAuthMethod::ScramSha256 => true,
        }
    }
}

fn default_db_auth_method() -> DbAuthMethod {
    DbAuthMethod::ScramSha256
}

#[derive(Debug, Deserialize, Clone)]
pub struct DbEmbeddedSqlite {
    /// Path to the sqlite database file when running embedded
    #[serde(default = "default_embedded_sqlite_path")]
    pub file_path: String,
    /// Optional maintenance vacuum interval in seconds
    #[serde(default)]
    pub vacuum_interval_seconds: Option<u64>,
}

impl Default for DbEmbeddedSqlite {
    fn default() -> Self {
        Self {
            file_path: default_embedded_sqlite_path(),
            vacuum_interval_seconds: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct DbEmbeddedPostgres {
    /// Data directory for embedded Postgres
    #[serde(default = "default_embedded_pg_data_dir")]
    pub data_dir: String,
    /// Path to the postgres binary (can be just "postgres" if on PATH)
    #[serde(default = "default_embedded_pg_binary")]
    pub binary_path: String,
    #[serde(default = "default_embedded_pg_port_range")]
    pub port_range: DbEmbeddedPortRange,
}

impl Default for DbEmbeddedPostgres {
    fn default() -> Self {
        Self {
            data_dir: default_embedded_pg_data_dir(),
            binary_path: default_embedded_pg_binary(),
            port_range: default_embedded_pg_port_range(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct DbEmbeddedPortRange {
    pub min: u16,
    pub max: u16,
}

fn default_embedded_sqlite_path() -> String {
    "runtime/db/fenrir.db".to_string()
}

fn default_embedded_pg_data_dir() -> String {
    "runtime/db/postgres".to_string()
}

fn default_embedded_pg_binary() -> String {
    "postgres".to_string()
}

fn default_embedded_pg_port_range() -> DbEmbeddedPortRange {
    DbEmbeddedPortRange {
        min: 55432,
        max: 55442,
    }
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
    #[serde(default)]
    pub history: TelemetryHistorySection,
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

#[derive(Debug, Deserialize, Clone, Default)]
pub struct TelemetryHistorySection {
    #[serde(default)]
    pub retention_days: Option<u64>,
    #[serde(default)]
    pub retention_hours: Option<u64>,
    #[serde(default)]
    pub persist_interval_seconds: Option<u64>,
    #[serde(default)]
    pub sample_interval_seconds: Option<u64>,
}

impl TelemetryHistorySection {
    pub fn retention_hours(&self) -> Option<u64> {
        if let Some(days) = self.retention_days {
            return days.checked_mul(24);
        }
        self.retention_hours
    }

    pub fn retention_seconds(&self) -> Option<u64> {
        self.retention_hours()
            .map(|hours| hours.saturating_mul(3600))
    }

    pub fn persist_interval_seconds(&self) -> Option<u64> {
        self.persist_interval_seconds
    }

    pub fn sample_interval_seconds(&self) -> Option<u64> {
        self.sample_interval_seconds
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
    pub retention_days: Option<u64>,
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
    #[serde(default)]
    pub dev_sources: ModuleDevSourcesSection,
    #[serde(default)]
    pub services: HashMap<String, ModuleServiceOverride>,
    #[serde(default)]
    pub service_profiles: HashMap<String, ModuleServiceProfile>,
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
    #[serde(default)]
    pub ports: ModuleRuntimePortSection,
    #[serde(default)]
    pub default_service_scopes: Vec<String>,
    #[serde(default)]
    pub clients: ModuleRuntimeClientSection,
    #[serde(default)]
    pub env_passthrough_prefixes: Vec<String>,
}

impl Default for ModuleRuntimeSection {
    fn default() -> Self {
        Self {
            engine: default_module_runtime_engine(),
            ports: ModuleRuntimePortSection::default(),
            default_service_scopes: Vec::new(),
            clients: ModuleRuntimeClientSection::default(),
            env_passthrough_prefixes: Vec::new(),
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
pub struct ModuleRuntimePortSection {
    #[serde(default = "default_module_port_strategy")]
    pub strategy: ModulePortStrategy,
    #[serde(default = "default_module_port_range")]
    pub range: ModulePortRange,
}

impl Default for ModuleRuntimePortSection {
    fn default() -> Self {
        Self {
            strategy: default_module_port_strategy(),
            range: default_module_port_range(),
        }
    }
}

impl ModuleRuntimePortSection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        match self.strategy {
            ModulePortStrategy::Dynamic => {
                if self.range.min == 0 || self.range.max == 0 {
                    return Err(ConfigError::Invalid(
                        "modules.runtime.ports.range min/max must be > 0",
                    ));
                }
                if self.range.min >= self.range.max {
                    return Err(ConfigError::Invalid(
                        "modules.runtime.ports.range.min must be less than range.max",
                    ));
                }
                if self.range.max.saturating_sub(self.range.min) < 10 {
                    return Err(ConfigError::Invalid(
                        "modules.runtime.ports.range must span at least 10 ports",
                    ));
                }
            }
            ModulePortStrategy::Fixed => {}
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModulePortStrategy {
    Dynamic,
    Fixed,
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct ModulePortRange {
    pub min: u16,
    pub max: u16,
}

impl ModulePortRange {
    pub fn contains(&self, port: u16) -> bool {
        port >= self.min && port <= self.max
    }
}

impl Default for ModulePortRange {
    fn default() -> Self {
        default_module_port_range()
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModuleStorageSection {
    pub install_dir: String,
    #[serde(default)]
    pub cache_dir: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ModuleRuntimeClientSection {
    #[serde(default = "default_client_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_client_retries")]
    pub retries: u32,
    #[serde(default = "default_client_backoff_ms")]
    pub backoff_ms: u64,
    #[serde(default = "default_health_probe_interval_secs")]
    pub health_probe_interval_seconds: u64,
    #[serde(default)]
    pub tls: ModuleRuntimeClientTlsSection,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ModuleRuntimeClientTlsSection {
    pub ca_cert_path: Option<String>,
    pub client_cert_path: Option<String>,
    pub client_key_path: Option<String>,
    #[serde(default)]
    pub accept_invalid_certs: bool,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ModuleServiceOverride {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub policy: ModuleServicePolicyOverride,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ModuleServicePolicyOverride {
    pub internal_only: Option<bool>,
    #[serde(default)]
    pub allowed_roles: Vec<String>,
    #[serde(default)]
    pub required_scopes: Vec<String>,
    #[serde(default)]
    pub tenant: Option<ModuleServiceTenantConfig>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ModuleServiceProfile {
    pub internal_only: Option<bool>,
    #[serde(default)]
    pub allowed_roles: Vec<String>,
    #[serde(default)]
    pub required_scopes: Vec<String>,
    #[serde(default)]
    pub tenant: Option<ModuleServiceTenantConfig>,
    #[serde(default)]
    pub ingress_access: Option<String>,
    #[serde(default)]
    pub rate_limit_per_second: Option<u32>,
    #[serde(default)]
    pub disable_rate_limit: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModuleServiceTenantConfig {
    #[serde(default = "default_tenant_mode")]
    pub mode: ModuleServiceTenantMode,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub allow: Vec<String>,
}

impl Default for ModuleServiceTenantConfig {
    fn default() -> Self {
        Self {
            mode: default_tenant_mode(),
            value: None,
            allow: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModuleServiceTenantMode {
    Any,
    Fixed,
    AllowList,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ModuleDevSourcesSection {
    #[serde(default)]
    pub base_path: Option<String>,
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

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ServiceTokenSection {
    #[serde(default = "default_service_token_lifetime_seconds")]
    pub lifetime_seconds: u64,
    #[serde(default = "default_service_token_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_service_token_cleanup_interval_seconds")]
    pub cleanup_interval_seconds: u64,
}

fn default_require_signature() -> bool {
    true
}

fn default_module_runtime_engine() -> ModuleRuntimeEngine {
    ModuleRuntimeEngine::Process
}

fn default_module_port_strategy() -> ModulePortStrategy {
    ModulePortStrategy::Dynamic
}

fn default_module_port_range() -> ModulePortRange {
    ModulePortRange {
        min: 41000,
        max: 46000,
    }
}

fn default_client_timeout_ms() -> u64 {
    10_000
}

fn default_client_retries() -> u32 {
    2
}

fn default_client_backoff_ms() -> u64 {
    200
}

fn default_health_probe_interval_secs() -> u64 {
    30
}

fn default_tenant_mode() -> ModuleServiceTenantMode {
    ModuleServiceTenantMode::Any
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

fn default_service_token_lifetime_seconds() -> u64 {
    600
}

fn default_service_token_idle_timeout_seconds() -> u64 {
    300
}

fn default_service_token_cleanup_interval_seconds() -> u64 {
    60
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
        self.resolve_control_tokens().map(|_| ())
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
            _ => {
                return Err(ConfigError::Invalid(
                    "security.kdf.algorithm must be argon2id",
                ))
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

impl ModuleRuntimeClientSection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.timeout_ms == 0 {
            return Err(ConfigError::Invalid(
                "modules.runtime.clients.timeout_ms must be > 0",
            ));
        }
        if self.backoff_ms == 0 {
            return Err(ConfigError::Invalid(
                "modules.runtime.clients.backoff_ms must be > 0",
            ));
        }
        if self.health_probe_interval_seconds == 0 {
            return Err(ConfigError::Invalid(
                "modules.runtime.clients.health_probe_interval_seconds must be > 0",
            ));
        }
        if self.tls.client_cert_path.is_some() ^ self.tls.client_key_path.is_some() {
            return Err(ConfigError::Invalid(
                "modules.runtime.clients.tls.client_cert_path and client_key_path must be set together",
            ));
        }
        Ok(())
    }
}

impl ModulesSection {
    pub fn validate_services(&self) -> Result<(), ConfigError> {
        for (service_id, override_cfg) in &self.services {
            override_cfg.validate(service_id)?;
        }
        Ok(())
    }
}

impl ModuleServiceOverride {
    fn validate(&self, service_id: &str) -> Result<(), ConfigError> {
        if service_id.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "modules.services keys must not be empty",
            ));
        }
        for key in self.env.keys().chain(self.secrets.keys()) {
            let trimmed = key.trim();
            if trimmed.is_empty() {
                return Err(ConfigError::InvalidMessage(format!(
                    "modules.services.{service_id} env keys must not be empty"
                )));
            }
            if trimmed.contains(char::is_whitespace) {
                return Err(ConfigError::InvalidMessage(format!(
                    "modules.services.{service_id} env key '{trimmed}' must not contain whitespace"
                )));
            }
        }
        if let Some(tenant) = &self.policy.tenant {
            tenant.validate(service_id)?;
        }
        Ok(())
    }
}

impl ModuleServiceTenantConfig {
    fn validate(&self, service_id: &str) -> Result<(), ConfigError> {
        match self.mode {
            ModuleServiceTenantMode::Any => Ok(()),
            ModuleServiceTenantMode::Fixed => {
                let value = self
                    .value
                    .as_ref()
                    .map(|v| v.trim())
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| {
                        ConfigError::InvalidMessage(format!(
                            "modules.services.{service_id}.policy.tenant.value must be set for mode=fixed"
                        ))
                    })?;
                if value.contains(char::is_whitespace) {
                    return Err(ConfigError::InvalidMessage(format!(
                        "modules.services.{service_id}.policy.tenant.value must not contain whitespace"
                    )));
                }
                Ok(())
            }
            ModuleServiceTenantMode::AllowList => {
                if self.allow.is_empty() {
                    return Err(ConfigError::InvalidMessage(format!(
                        "modules.services.{service_id}.policy.tenant.allow must list at least one tenant for mode=allow_list"
                    )));
                }
                if self
                    .allow
                    .iter()
                    .any(|value| value.trim().is_empty() || value.contains(char::is_whitespace))
                {
                    return Err(ConfigError::InvalidMessage(format!(
                        "modules.services.{service_id}.policy.tenant.allow entries must be non-empty and without whitespace"
                    )));
                }
                Ok(())
            }
        }
    }
}

impl ModuleRuntimeSection {
    pub fn validate(&self) -> Result<(), ConfigError> {
        match self.engine {
            ModuleRuntimeEngine::Process | ModuleRuntimeEngine::Stub => {}
        }
        self.ports.validate()?;
        self.clients.validate()?;
        for prefix in &self.env_passthrough_prefixes {
            let value = prefix.trim();
            if value.is_empty() {
                return Err(ConfigError::Invalid(
                    "modules.runtime.env_passthrough_prefixes must not contain empty values",
                ));
            }
            if !value.ends_with('_') {
                return Err(ConfigError::InvalidMessage(format!(
                    "modules.runtime.env_passthrough_prefixes entry '{value}' must end with '_'"
                )));
            }
            if value
                .chars()
                .any(|ch| !ch.is_ascii_uppercase() && !ch.is_ascii_digit() && ch != '_')
            {
                return Err(ConfigError::InvalidMessage(format!(
                    "modules.runtime.env_passthrough_prefixes entry '{value}' must contain only A-Z, 0-9 and '_'"
                )));
            }
        }
        Ok(())
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
        if let Some(days) = self.retention_days {
            if days == 0 {
                return Err(ConfigError::Invalid(
                    "audit.storage.retention_days must be > 0 when set",
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
        if let Some(days) = self.retention_days {
            return days.checked_mul(24);
        }
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
        if let Some(days) = self.history.retention_days {
            if days == 0 {
                return Err(ConfigError::Invalid(
                    "telemetry.history.retention_days must be > 0 when set",
                ));
            }
        }
        if let Some(hours) = self.history.retention_hours {
            if hours == 0 {
                return Err(ConfigError::Invalid(
                    "telemetry.history.retention_hours must be > 0 when set",
                ));
            }
        }
        if let Some(interval) = self.history.persist_interval_seconds {
            if interval == 0 {
                return Err(ConfigError::Invalid(
                    "telemetry.history.persist_interval_seconds must be > 0 when set",
                ));
            }
        }
        if let Some(interval) = self.history.sample_interval_seconds {
            if interval == 0 {
                return Err(ConfigError::Invalid(
                    "telemetry.history.sample_interval_seconds must be > 0 when set",
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

impl DbRuntimeSection {
    pub fn validate(&self, default_engine: &str) -> Result<(), ConfigError> {
        match self.mode {
            DbRuntimeMode::External => Ok(()),
            DbRuntimeMode::Embedded => self.embedded.validate(default_engine),
        }
    }
}

impl DbEmbeddedSection {
    fn validate(&self, default_engine: &str) -> Result<(), ConfigError> {
        if default_engine != self.engine.as_str() {
            return Err(ConfigError::Invalid(
                "db.runtime.embedded.engine must match db.default_engine when mode = embedded",
            ));
        }
        match self.engine {
            EmbeddedEngineKind::Sqlite => self.sqlite.validate(),
            EmbeddedEngineKind::Postgres => self.postgres.validate(),
        }
    }
}

impl DbEmbeddedSqlite {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.file_path.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "db.runtime.embedded.sqlite.file_path must not be empty",
            ));
        }
        if let Some(interval) = self.vacuum_interval_seconds {
            if interval == 0 {
                return Err(ConfigError::Invalid(
                    "db.runtime.embedded.sqlite.vacuum_interval_seconds must be > 0 when set",
                ));
            }
        }
        Ok(())
    }
}

impl DbEmbeddedPostgres {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.data_dir.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "db.runtime.embedded.postgres.data_dir must not be empty",
            ));
        }
        if self.binary_path.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "db.runtime.embedded.postgres.binary_path must not be empty",
            ));
        }
        self.port_range.validate()?;
        Ok(())
    }
}

impl DbEmbeddedPortRange {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.min == 0 || self.max == 0 {
            return Err(ConfigError::Invalid(
                "db.runtime.embedded.postgres.port_range min/max must be > 0",
            ));
        }
        if self.min > self.max {
            return Err(ConfigError::Invalid(
                "db.runtime.embedded.postgres.port_range.min must be <= max",
            ));
        }
        Ok(())
    }
}

fn parse_role_token(raw: &str) -> Result<(Role, &str), ConfigError> {
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
    let parsed_role = Role::from_str(role).map_err(|_| {
        ConfigError::Invalid(
            "security.http.control_tokens must prefix role as admin|operator|viewer",
        )
    })?;
    Ok((parsed_role, value))
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
