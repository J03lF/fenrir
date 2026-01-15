//! Backup types and error definitions.

use std::path::PathBuf;
use thiserror::Error;
use time::OffsetDateTime;

/// Backup operation result
pub type BackupResult<T> = Result<T, BackupError>;

/// Backup operation errors
#[derive(Debug, Error)]
pub enum BackupError {
    #[error("database not healthy: {0}")]
    DatabaseUnhealthy(String),

    #[error("not enough disk space: {available_mb} MB available, {required_mb} MB required")]
    InsufficientDiskSpace { available_mb: u64, required_mb: u64 },

    #[error("database has active locks, backup skipped")]
    ActiveLocks,

    #[error("another backup is already in progress")]
    BackupInProgress,

    #[error("backup size anomaly detected: expected ~{expected_mb} MB, got {actual_mb} MB ({deviation_pct}% deviation)")]
    SizeAnomaly {
        expected_mb: u64,
        actual_mb: u64,
        deviation_pct: u32,
    },

    #[error("backup integrity check failed: {0}")]
    IntegrityCheckFailed(String),

    #[error("checkpoint failed: {0}")]
    CheckpointFailed(String),

    #[error("backup command failed: {0}")]
    BackupFailed(String),

    #[error("rotation failed: {0}")]
    RotationFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("backup not enabled in configuration")]
    NotEnabled,

    #[error("db runtime not available")]
    DbRuntimeNotAvailable,
}

/// Status of a backup operation
#[derive(Debug, Clone)]
pub struct BackupStatus {
    /// Backup file/directory path
    pub path: PathBuf,
    /// When the backup was created
    pub created_at: OffsetDateTime,
    /// Size in bytes
    pub size_bytes: u64,
    /// SHA256 checksum (if verification enabled)
    pub checksum: Option<String>,
    /// Database engine
    pub engine: String,
    /// Whether integrity was verified
    pub verified: bool,
}

/// What triggered the backup run
#[derive(Debug, Clone, Copy)]
pub enum BackupTrigger {
    Auto,
}

impl BackupTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            BackupTrigger::Auto => "auto",
        }
    }
}

/// Backup metadata stored alongside backups
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackupMetadata {
    pub created_at: String,
    pub engine: String,
    pub size_bytes: u64,
    pub checksum: Option<String>,
    pub verified: bool,
    pub fenrir_version: String,
}

/// Information about available backups
#[derive(Debug, Clone)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub metadata: Option<BackupMetadata>,
    pub size_bytes: u64,
    pub created_at: OffsetDateTime,
}

/// Pre-backup check results
#[derive(Debug)]
pub struct PreBackupChecks {
    pub db_healthy: bool,
    pub disk_space_mb: u64,
    pub has_active_locks: bool,
    pub last_backup_size_mb: Option<u64>,
}

