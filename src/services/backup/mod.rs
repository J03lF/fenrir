//! Database backup service with health guards and integrity checks.
//!
//! Implements the backup flow:
//! 1. Health-Check (DB reachable, no locks, disk space)
//! 2. Pre-Backup (checkpoint, anomaly detection)
//! 3. Backup (pg_basebackup / sqlite copy)
//! 4. Post-Backup (checksum, rotation, audit)

mod service;
mod types;

pub use service::BackupService;
pub use types::{BackupError, BackupResult, BackupStatus, BackupTrigger};

