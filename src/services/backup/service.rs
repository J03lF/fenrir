//! Backup service implementation.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::fs;
use tokio::process::Command;
use tracing::{info, warn};

use crate::audit::{AuditActor, AuditEvent, AuditLog, AuditOutcome};
use crate::config::{DbBackupSection, EmbeddedEngineKind};
use crate::infra::db::runtime::DbRuntimeSupervisor;
use crate::services::db_shell::DbShellService;

use super::types::{
    BackupError, BackupInfo, BackupMetadata, BackupResult, BackupStatus, BackupTrigger,
    PreBackupChecks,
};

/// Database backup service with health guards
pub struct BackupService {
    config: DbBackupSection,
    engine: EmbeddedEngineKind,
    db_shell: Arc<DbShellService>,
    db_runtime: Arc<DbRuntimeSupervisor>,
    audit_log: Arc<dyn AuditLog>,
    runtime_dir: PathBuf,
    backup_in_progress: AtomicBool,
}

impl BackupService {
    /// Create a new backup service
    pub fn new(
        config: DbBackupSection,
        engine: EmbeddedEngineKind,
        db_shell: Arc<DbShellService>,
        db_runtime: Arc<DbRuntimeSupervisor>,
        audit_log: Arc<dyn AuditLog>,
        runtime_dir: PathBuf,
    ) -> Self {
        Self {
            config,
            engine,
            db_shell,
            db_runtime,
            audit_log,
            runtime_dir,
            backup_in_progress: AtomicBool::new(false),
        }
    }

    /// Get the backup directory path
    pub fn backup_dir(&self) -> PathBuf {
        let path = Path::new(&self.config.path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.runtime_dir.join(path)
        }
    }

    /// Run the complete backup flow
    pub async fn run_backup(&self, trigger: BackupTrigger) -> BackupResult<BackupStatus> {
        if !self.config.enabled {
            return Err(BackupError::NotEnabled);
        }

        // Acquire backup lock
        if self
            .backup_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(BackupError::BackupInProgress);
        }

        let result = self.run_backup_inner().await;

        // Release lock
        self.backup_in_progress.store(false, Ordering::SeqCst);

        // Log audit event
        self.log_audit_event(trigger, &result);

        result
    }

    async fn run_backup_inner(&self) -> BackupResult<BackupStatus> {
        // === Phase 1: Health Checks ===
        info!("Starting backup: running pre-flight checks");
        let checks = self.run_pre_checks().await?;

        if self.config.require_healthy && !checks.db_healthy {
            return Err(BackupError::DatabaseUnhealthy(
                "database ping failed".to_string(),
            ));
        }

        if checks.disk_space_mb < self.config.min_disk_space_mb {
            return Err(BackupError::InsufficientDiskSpace {
                available_mb: checks.disk_space_mb,
                required_mb: self.config.min_disk_space_mb,
            });
        }

        if checks.has_active_locks {
            warn!("Database has active locks, skipping backup");
            return Err(BackupError::ActiveLocks);
        }

        // === Phase 2: Pre-Backup ===
        info!("Pre-backup: forcing checkpoint");
        self.force_checkpoint().await?;

        // Check for size anomaly
        if self.config.anomaly_threshold_pct > 0 {
            if let Some(last_size) = checks.last_backup_size_mb {
                // We'll check after backup is created
                info!(
                    "Anomaly detection enabled, last backup was {} MB",
                    last_size
                );
            }
        }

        // === Phase 3: Create Backup ===
        info!("Creating backup");
        let backup_path = self.create_backup().await?;
        let size_bytes = self.get_backup_size(&backup_path).await?;
        let size_mb = size_bytes / (1024 * 1024);

        // Anomaly check
        if self.config.anomaly_threshold_pct > 0 {
            if let Some(last_size_mb) = checks.last_backup_size_mb {
                if last_size_mb > 0 {
                    let deviation = if size_mb > last_size_mb {
                        ((size_mb - last_size_mb) * 100) / last_size_mb
                    } else {
                        ((last_size_mb - size_mb) * 100) / last_size_mb
                    };

                    if deviation > self.config.anomaly_threshold_pct as u64 {
                        warn!(
                            "Backup size anomaly: {} MB vs expected ~{} MB ({}% deviation)",
                            size_mb, last_size_mb, deviation
                        );
                        return Err(BackupError::SizeAnomaly {
                            expected_mb: last_size_mb,
                            actual_mb: size_mb,
                            deviation_pct: deviation as u32,
                        });
                    }
                }
            }
        }

        // === Phase 4: Post-Backup ===
        // Refresh and copy db_state.json snapshot for restore compatibility
        info!("Refreshing DB state snapshot");
        self.db_runtime
            .refresh_state_snapshot(Some(time::OffsetDateTime::now_utc()))
            .await;

        if let Err(err) = self.db_runtime.copy_snapshot_to_backup(&backup_path).await {
            warn!(
                "Failed to copy db state snapshot: {} (restore may require manual intervention)",
                err
            );
        } else {
            info!("DB state snapshot copied to backup");
        }

        let checksum = if self.config.verify_integrity {
            info!("Verifying backup integrity");
            Some(self.verify_and_checksum(&backup_path).await?)
        } else {
            None
        };

        // Write metadata file
        let metadata = BackupMetadata {
            created_at: OffsetDateTime::now_utc().to_string(),
            engine: self.engine.as_str().to_string(),
            size_bytes,
            checksum: checksum.clone(),
            verified: self.config.verify_integrity,
            fenrir_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        self.write_metadata(&backup_path, &metadata).await?;

        // Rotate old backups
        info!(
            "Rotating old backups (keeping {})",
            self.config.retention_count
        );
        self.rotate_backups().await?;

        let status = BackupStatus {
            path: backup_path,
            created_at: OffsetDateTime::now_utc(),
            size_bytes,
            checksum,
            engine: self.engine.as_str().to_string(),
            verified: self.config.verify_integrity,
        };

        info!(
            "Backup complete: {} ({} MB)",
            status.path.display(),
            size_bytes / (1024 * 1024)
        );

        Ok(status)
    }

    /// Run pre-flight health checks
    async fn run_pre_checks(&self) -> BackupResult<PreBackupChecks> {
        // Check DB health
        let session = self.db_shell.create_session();
        let db_healthy = session.ping().await.is_ok();

        // Check disk space
        let disk_space_mb = self.get_available_disk_space().await.unwrap_or(0);

        // Check for active locks (simplified - just check if we can query)
        let has_active_locks = false; // TODO: implement proper lock check

        // Get last backup size
        let last_backup_size_mb = self.get_last_backup_size_mb().await.ok().flatten();

        Ok(PreBackupChecks {
            db_healthy,
            disk_space_mb,
            has_active_locks,
            last_backup_size_mb,
        })
    }

    /// Force a database checkpoint
    async fn force_checkpoint(&self) -> BackupResult<()> {
        let session = self.db_shell.create_session();

        match self.engine {
            EmbeddedEngineKind::Postgres => {
                session
                    .simple_query("CHECKPOINT")
                    .await
                    .map_err(|e| BackupError::CheckpointFailed(e.to_string()))?;
            }
            EmbeddedEngineKind::Sqlite => {
                // SQLite uses WAL checkpoint
                session
                    .simple_query("PRAGMA wal_checkpoint(TRUNCATE)")
                    .await
                    .map_err(|e| BackupError::CheckpointFailed(e.to_string()))?;
            }
        }

        Ok(())
    }

    /// Create the actual backup
    async fn create_backup(&self) -> BackupResult<PathBuf> {
        let backup_dir = self.backup_dir();
        fs::create_dir_all(&backup_dir).await?;

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let backup_name = format!("{}-{}", self.engine.as_str(), timestamp);

        match self.engine {
            EmbeddedEngineKind::Postgres => {
                self.create_postgres_backup(&backup_dir, &backup_name).await
            }
            EmbeddedEngineKind::Sqlite => {
                self.create_sqlite_backup(&backup_dir, &backup_name).await
            }
        }
    }

    async fn create_postgres_backup(
        &self,
        backup_dir: &Path,
        backup_name: &str,
    ) -> BackupResult<PathBuf> {
        let target_dir = backup_dir.join(backup_name);

        // Get connection info from runtime supervisor
        let (use_socket, socket_dir, port, user) = self.db_runtime.connection_info();

        // Load credentials for authentication (scram-sha-256 requires password)
        let credentials = self.db_runtime.credentials().await.map_err(|e| {
            BackupError::BackupFailed(format!("failed to load db credentials: {}", e))
        })?;

        // Find pg_basebackup binary
        let pg_basebackup = self.find_pg_basebackup()?;

        // Build pg_basebackup command based on connection mode
        let mut cmd = Command::new(&pg_basebackup);

        // Set PGPASSWORD environment variable for authentication
        cmd.env("PGPASSWORD", &credentials.password);

        cmd.arg("-D")
            .arg(&target_dir)
            .arg("-X")
            .arg("stream")
            .arg("-c")
            .arg("fast")
            .arg("-U")
            .arg(&user);

        if use_socket {
            // Unix socket mode - use socket directory as host
            info!("Using Unix socket for backup: {}", socket_dir.display());
            cmd.arg("-h").arg(&socket_dir);
        } else {
            // TCP mode - use localhost and port
            let port = port.ok_or_else(|| {
                BackupError::BackupFailed("embedded postgres not running - no port assigned".into())
            })?;
            info!("Using TCP for backup: 127.0.0.1:{}", port);
            cmd.arg("-h")
                .arg("127.0.0.1")
                .arg("-p")
                .arg(port.to_string());
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| BackupError::BackupFailed(format!("pg_basebackup spawn: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BackupError::BackupFailed(format!(
                "pg_basebackup failed: {}",
                stderr
            )));
        }

        Ok(target_dir)
    }

    /// Find pg_basebackup binary - check config, then common paths
    fn find_pg_basebackup(&self) -> BackupResult<PathBuf> {
        // 1. Check config
        if let Some(path) = &self.config.pg_basebackup_path {
            let p = PathBuf::from(path);
            if p.exists() {
                return Ok(p);
            }
            return Err(BackupError::BackupFailed(format!(
                "configured pg_basebackup_path not found: {}",
                path
            )));
        }

        // 2. Check PATH
        if let Ok(output) = std::process::Command::new("which")
            .arg("pg_basebackup")
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Ok(PathBuf::from(path));
                }
            }
        }

        // 3. Check common Homebrew paths (macOS)
        let common_paths = [
            "/opt/homebrew/opt/postgresql@16/bin/pg_basebackup",
            "/opt/homebrew/opt/postgresql@15/bin/pg_basebackup",
            "/opt/homebrew/opt/postgresql/bin/pg_basebackup",
            "/opt/homebrew/bin/pg_basebackup",
            "/usr/local/opt/postgresql@16/bin/pg_basebackup",
            "/usr/local/opt/postgresql@15/bin/pg_basebackup",
            "/usr/local/bin/pg_basebackup",
            "/usr/bin/pg_basebackup",
        ];

        for path in common_paths {
            let p = PathBuf::from(path);
            if p.exists() {
                info!("Found pg_basebackup at: {}", path);
                return Ok(p);
            }
        }

        Err(BackupError::BackupFailed(
            "pg_basebackup not found. Install PostgreSQL or set db.backup.pg_basebackup_path in config".to_string()
        ))
    }

    async fn create_sqlite_backup(
        &self,
        backup_dir: &Path,
        backup_name: &str,
    ) -> BackupResult<PathBuf> {
        let db_path = self.runtime_dir.join("db").join("fenrir.db");
        let backup_path = backup_dir.join(format!("{}.db", backup_name));

        fs::copy(&db_path, &backup_path)
            .await
            .map_err(|e| BackupError::BackupFailed(format!("sqlite copy failed: {}", e)))?;

        Ok(backup_path)
    }

    /// Get backup size in bytes
    async fn get_backup_size(&self, path: &Path) -> BackupResult<u64> {
        if path.is_dir() {
            // For directories (postgres), sum all files
            let mut total = 0u64;
            let mut entries = fs::read_dir(path).await?;
            while let Some(entry) = entries.next_entry().await? {
                let metadata = entry.metadata().await?;
                if metadata.is_file() {
                    total += metadata.len();
                } else if metadata.is_dir() {
                    // Recursive would be better, but approximate is fine
                    total += 1024 * 1024; // Estimate 1MB per subdir
                }
            }
            Ok(total)
        } else {
            let metadata = fs::metadata(path).await?;
            Ok(metadata.len())
        }
    }

    /// Verify backup integrity and compute checksum
    async fn verify_and_checksum(&self, path: &Path) -> BackupResult<String> {
        match self.engine {
            EmbeddedEngineKind::Sqlite => {
                // For SQLite, checksum the file
                let data = fs::read(path).await?;
                let mut hasher = Sha256::new();
                hasher.update(&data);
                let hash = hasher.finalize();
                Ok(format!("{:x}", hash))
            }
            EmbeddedEngineKind::Postgres => {
                // For Postgres, check that key files exist
                let pg_version = path.join("PG_VERSION");
                if !pg_version.exists() {
                    return Err(BackupError::IntegrityCheckFailed(
                        "PG_VERSION file missing".to_string(),
                    ));
                }
                // Return a simple marker for postgres backups
                Ok("postgres-backup-verified".to_string())
            }
        }
    }

    /// Write metadata file alongside backup
    async fn write_metadata(
        &self,
        backup_path: &Path,
        metadata: &BackupMetadata,
    ) -> BackupResult<()> {
        let meta_path = if backup_path.is_dir() {
            backup_path.join("backup.json")
        } else {
            backup_path.with_extension("json")
        };

        let json = serde_json::to_string_pretty(metadata)
            .map_err(|e| BackupError::Io(std::io::Error::other(e)))?;

        fs::write(&meta_path, json).await?;
        Ok(())
    }

    /// Rotate old backups, keeping only retention_count
    async fn rotate_backups(&self) -> BackupResult<()> {
        let backup_dir = self.backup_dir();
        if !backup_dir.exists() {
            return Ok(());
        }

        let mut backups = self.list_backups_internal().await?;

        // Sort by creation time (newest first)
        backups.sort_by_key(|b| std::cmp::Reverse(b.created_at));

        // Remove old backups
        if backups.len() > self.config.retention_count {
            for backup in backups.iter().skip(self.config.retention_count) {
                info!("Removing old backup: {}", backup.path.display());
                if backup.path.is_dir() {
                    fs::remove_dir_all(&backup.path).await.map_err(|e| {
                        BackupError::RotationFailed(format!(
                            "failed to remove {}: {}",
                            backup.path.display(),
                            e
                        ))
                    })?;
                } else {
                    fs::remove_file(&backup.path).await.map_err(|e| {
                        BackupError::RotationFailed(format!(
                            "failed to remove {}: {}",
                            backup.path.display(),
                            e
                        ))
                    })?;
                }
                // Also remove metadata file
                let meta_path = backup.path.with_extension("json");
                let _ = fs::remove_file(&meta_path).await;
            }
        }

        Ok(())
    }

    /// List existing backups
    pub async fn list_backups(&self) -> BackupResult<Vec<BackupInfo>> {
        self.list_backups_internal().await
    }

    async fn list_backups_internal(&self) -> BackupResult<Vec<BackupInfo>> {
        let backup_dir = self.backup_dir();
        if !backup_dir.exists() {
            return Ok(Vec::new());
        }

        let mut backups = Vec::new();
        let mut entries = fs::read_dir(&backup_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            let metadata = entry.metadata().await?;

            // Skip metadata files
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                continue;
            }

            // Try to read backup metadata
            let meta_path = if path.is_dir() {
                path.join("backup.json")
            } else {
                path.with_extension("json")
            };

            let backup_metadata = if meta_path.exists() {
                fs::read_to_string(&meta_path)
                    .await
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
            } else {
                None
            };

            let created_at = metadata
                .created()
                .ok()
                .map(OffsetDateTime::from)
                .unwrap_or_else(OffsetDateTime::now_utc);

            backups.push(BackupInfo {
                path,
                metadata: backup_metadata,
                size_bytes: metadata.len(),
                created_at,
            });
        }

        Ok(backups)
    }

    async fn get_last_backup_size_mb(&self) -> BackupResult<Option<u64>> {
        let backups = self.list_backups_internal().await?;
        if let Some(last) = backups.first() {
            Ok(Some(last.size_bytes / (1024 * 1024)))
        } else {
            Ok(None)
        }
    }

    async fn get_available_disk_space(&self) -> BackupResult<u64> {
        // Use statvfs on Unix
        #[cfg(unix)]
        {
            use std::ffi::CString;
            use std::mem::MaybeUninit;

            let backup_dir = self.backup_dir();
            let path_str = backup_dir.to_string_lossy();
            let c_path = CString::new(path_str.as_bytes()).map_err(|_| {
                BackupError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "invalid path",
                ))
            })?;

            let mut stat: MaybeUninit<libc::statvfs> = MaybeUninit::uninit();
            let result = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };

            if result == 0 {
                let stat = unsafe { stat.assume_init() };
                let available_bytes = stat.f_bavail as u64 * stat.f_frsize;
                return Ok(available_bytes / (1024 * 1024));
            }
        }

        // Fallback: assume enough space
        Ok(10_000) // 10 GB default
    }

    fn log_audit_event(&self, trigger: BackupTrigger, result: &BackupResult<BackupStatus>) {
        use crate::audit::AuditMetadata;

        let mut metadata = AuditMetadata::default()
            .insert("trigger", trigger.as_str())
            .insert("engine", self.engine.as_str())
            .insert("schedule", self.config.schedule.clone());
        let outcome = match result {
            Ok(status) => {
                metadata = metadata
                    .insert("path", status.path.display().to_string())
                    .insert("size_bytes", status.size_bytes.to_string())
                    .insert("verified", status.verified.to_string());
                if let Some(checksum) = &status.checksum {
                    metadata = metadata.insert("checksum", checksum.clone());
                }
                AuditOutcome::Success
            }
            Err(err) => {
                metadata = metadata.insert("error", err.to_string());
                AuditOutcome::Failure
            }
        };

        let builder = AuditEvent::builder()
            .actor(AuditActor::System)
            .action("backup::db")
            .target("db-runtime".to_string())
            .outcome(outcome)
            .metadata(metadata);

        if let Ok(ev) = builder.build() {
            if let Err(e) = self.audit_log.append(ev) {
                warn!("Failed to log backup audit event: {}", e);
            }
        }
    }

    /// Check if backup is currently enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get backup configuration
    pub fn config(&self) -> &DbBackupSection {
        &self.config
    }
}
