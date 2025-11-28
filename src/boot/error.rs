use std::fmt;

use anyhow::Error as AnyError;
use thiserror::Error;

use crate::config::ConfigError;
use crate::utils::messages::boot::errors as boot_errors;

#[derive(Debug, Clone, Copy)]
pub enum BootErrorCode {
    ConfigLoad,
    ConfigInvalid,
    ConfigMissingSecret,
    LoggingInit,
    TelemetryInit,
    TelemetryProbe,
    DbAdapters,
    DbEngine,
    DbShellInit,
    AuditInit,
    ModuleRegistry,
    ModuleStorage,
    ModuleVerifier,
    ModuleAttach,
    IdentityInit,
    SecurityInit,
    SecurityAttach,
    SessionAttach,
    SchedulerJobs,
    HttpServerInit,
}

impl BootErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            BootErrorCode::ConfigLoad => "BOOT-CONFIG-LOAD",
            BootErrorCode::ConfigInvalid => "BOOT-CONFIG-INVALID",
            BootErrorCode::ConfigMissingSecret => "BOOT-CONFIG-MISSING-SECRET",
            BootErrorCode::LoggingInit => "BOOT-LOGGING-INIT",
            BootErrorCode::TelemetryInit => "BOOT-TELEMETRY-INIT",
            BootErrorCode::TelemetryProbe => "BOOT-TELEMETRY-PROBE",
            BootErrorCode::DbAdapters => "BOOT-DB-ADAPTERS",
            BootErrorCode::DbEngine => "BOOT-DB-ENGINE",
            BootErrorCode::DbShellInit => "BOOT-DB-SHELL",
            BootErrorCode::AuditInit => "BOOT-AUDIT",
            BootErrorCode::ModuleRegistry => "BOOT-MODULE-REGISTRY",
            BootErrorCode::ModuleStorage => "BOOT-MODULE-STORAGE",
            BootErrorCode::ModuleVerifier => "BOOT-MODULE-VERIFIER",
            BootErrorCode::ModuleAttach => "BOOT-MODULE-ATTACH",
            BootErrorCode::IdentityInit => "BOOT-IDENTITY-INIT",
            BootErrorCode::SecurityInit => "BOOT-SECURITY-INIT",
            BootErrorCode::SecurityAttach => "BOOT-SECURITY-ATTACH",
            BootErrorCode::SessionAttach => "BOOT-SESSION-ATTACH",
            BootErrorCode::SchedulerJobs => "BOOT-SCHEDULER-JOBS",
            BootErrorCode::HttpServerInit => "BOOT-HTTP-INIT",
        }
    }
}

impl fmt::Display for BootErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct BootError {
    code: BootErrorCode,
    message: &'static str,
    #[source]
    source: AnyError,
}

impl BootError {
    pub fn new(code: BootErrorCode, message: &'static str, source: impl Into<AnyError>) -> Self {
        Self {
            code,
            message,
            source: source.into(),
        }
    }

    pub fn from_config(err: ConfigError) -> Self {
        match err {
            ConfigError::MissingEnv { .. } => BootError::new(
                BootErrorCode::ConfigMissingSecret,
                boot_errors::MISSING_SECRET,
                err,
            ),
            ConfigError::Invalid(_) => BootError::new(
                BootErrorCode::ConfigInvalid,
                boot_errors::CONFIG_INVALID,
                err,
            ),
            ConfigError::MissingConfigFile { .. } => BootError::new(
                BootErrorCode::ConfigLoad,
                boot_errors::CONFIG_FILE_NOT_FOUND,
                err,
            ),
            ConfigError::InvalidProfile { .. } => BootError::new(
                BootErrorCode::ConfigInvalid,
                boot_errors::CONFIG_PROFILE_INVALID,
                err,
            ),
            ConfigError::Anyhow(_) => BootError::new(
                BootErrorCode::ConfigLoad,
                boot_errors::CONFIG_LOAD_FAILED,
                err,
            ),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code.as_str()
    }

    pub fn message(&self) -> &'static str {
        self.message
    }
}

pub(crate) fn wrap_boot<T, E>(
    result: Result<T, E>,
    code: BootErrorCode,
    message: &'static str,
) -> Result<T, BootError>
where
    AnyError: From<E>,
{
    result.map_err(|err| BootError::new(code, message, err))
}
