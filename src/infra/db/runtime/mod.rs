use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use once_cell::sync::OnceCell;
use time::OffsetDateTime;
use tokio::fs;
use tracing::{info, warn};
use urlencoding;

use crate::config::{
    DbEmbeddedPortRange, DbEmbeddedSecuritySection, DbRuntimeMode, EmbeddedEngineKind,
};
use crate::infra::telemetry;
use crate::security::manager::SecurityManager;
use crate::services::{ManagedService, ServiceDiagnostics};

mod credentials;
use credentials::{DbCredentials, SealedDbCredentials};
mod snapshots;
use snapshots::{backup_postgres_with_auth, backup_sqlite, restore_postgres, restore_sqlite};
pub(crate) mod state;
use state::{
    collect_snapshot_with_health, load_snapshot, persist_snapshot, AdapterHealthState,
    AdapterHealthStatus,
};

/// How long to wait for postgres to expose its socket / port.
const PG_SOCKET_WAIT: Duration = Duration::from_secs(10);

/// How long to wait for postgres to complete a handshake once it listens.
/// Generous on purpose: a cold start that replays WAL can sit in
/// "starting up" for several seconds on a slow disk.
const PG_HANDSHAKE_WAIT: Duration = Duration::from_secs(30);

/// Gap between readiness attempts.
const PG_PROBE_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct DbBackupArtifact {
    pub artifact_path: String,
    pub state_snapshot_path: Option<String>,
}

/// Progress updates during database restore
#[derive(Debug, Clone)]
pub enum RestoreProgress {
    /// Stopping the database server
    StoppingServer,
    /// Server stopped successfully
    ServerStopped,
    /// Copying backup data
    RestoringData,
    /// Data restored successfully
    DataRestored,
    /// Cleaning up recovery files
    CleaningUp,
    /// Cleanup complete
    CleanupComplete,
    /// Starting the database server
    StartingServer,
    /// Server started successfully
    ServerStarted,
    /// Restore complete
    Complete,
}

/// Progress updates during database backup
#[derive(Debug, Clone)]
pub enum BackupProgress {
    /// Preparing backup
    Preparing,
    /// Running pg_basebackup or copying files
    CreatingBackup,
    /// Backup data created
    BackupCreated,
    /// Saving state snapshot
    SavingState,
    /// State saved
    StateSaved,
    /// Backup complete
    Complete,
}

/// Embedded database runtime supervisor.
/// Can be created early in boot (without SecurityManager/Diagnostics) and services attached later.
pub struct DbRuntimeSupervisor {
    mode: DbRuntimeMode,
    engine: EmbeddedEngineKind,
    port_range: DbEmbeddedPortRange,
    pg_binary: String,
    runtime_dir: PathBuf,
    running: AtomicBool,
    connector_uri: RwLock<Option<String>>,
    /// Late-bound security manager (for credential sealing).
    security: OnceCell<Arc<SecurityManager>>,
    /// Late-bound diagnostics (for probes/heartbeats).
    diagnostics: OnceCell<Arc<ServiceDiagnostics>>,
    status: RwLock<RuntimeStatus>,
    pg_process: tokio::sync::Mutex<Option<PostgresProcess>>,
    /// Security settings for the embedded database.
    db_security: DbEmbeddedSecuritySection,
}

impl DbRuntimeSupervisor {
    /// Create supervisor for early boot (before SecurityManager/Diagnostics are available).
    pub fn new_early(
        mode: DbRuntimeMode,
        engine: EmbeddedEngineKind,
        port_range: DbEmbeddedPortRange,
        pg_binary: String,
        runtime_dir: PathBuf,
        db_security: DbEmbeddedSecuritySection,
    ) -> Arc<Self> {
        Arc::new(Self {
            mode,
            engine,
            port_range,
            pg_binary,
            runtime_dir,
            running: AtomicBool::new(false),
            connector_uri: RwLock::new(None),
            security: OnceCell::new(),
            diagnostics: OnceCell::new(),
            status: RwLock::new(RuntimeStatus::default(engine)),
            pg_process: tokio::sync::Mutex::new(None),
            db_security,
        })
    }

    /// Create supervisor with all dependencies (for late creation in boot).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mode: DbRuntimeMode,
        engine: EmbeddedEngineKind,
        port_range: DbEmbeddedPortRange,
        pg_binary: String,
        runtime_dir: PathBuf,
        db_security: DbEmbeddedSecuritySection,
        security: Arc<SecurityManager>,
        diagnostics: Arc<ServiceDiagnostics>,
    ) -> Arc<Self> {
        let sup = Self::new_early(
            mode,
            engine,
            port_range,
            pg_binary,
            runtime_dir,
            db_security,
        );
        let _ = sup.security.set(security);
        let _ = sup.diagnostics.set(diagnostics);
        sup
    }

    /// Attach SecurityManager (for credential sealing). Call after early creation.
    pub fn attach_security(&self, security: Arc<SecurityManager>) {
        let _ = self.security.set(security);
    }

    /// Attach ServiceDiagnostics (for probes/heartbeats). Call after early creation.
    pub fn attach_diagnostics(&self, diagnostics: Arc<ServiceDiagnostics>) {
        let _ = self.diagnostics.set(diagnostics);
    }

    /// Returns the connector URI (stubbed for embedded sqlite).
    pub fn connector_uri(&self) -> Option<String> {
        self.connector_uri.read().ok().and_then(|uri| uri.clone())
    }

    /// Load (or generate) embedded DB credentials.
    pub async fn credentials(&self) -> anyhow::Result<DbCredentials> {
        let security = self.security.get().ok_or_else(|| {
            anyhow::anyhow!("SecurityManager not attached to db-runtime supervisor")
        })?;
        SealedDbCredentials::load_or_create(security, &self.runtime_dir).await
    }

    fn set_connector_uri(&self, uri: String) {
        if let Ok(mut guard) = self.connector_uri.write() {
            *guard = Some(uri.clone());
        }
        self.update_status(|s| s.connector_uri = Some(uri));
    }

    pub fn status_snapshot(&self) -> Option<RuntimeStatus> {
        self.status.read().ok().map(|g| g.clone())
    }

    /// Get connection info for pg_basebackup and similar tools.
    /// Returns (use_socket, socket_dir, port, user).
    pub fn connection_info(&self) -> (bool, PathBuf, Option<u16>, String) {
        let use_socket = self.db_security.prefer_unix_socket;
        let socket_dir = self.runtime_dir.join("socket");
        let port = self.status.read().ok().and_then(|s| s.port);
        let user = "fenrir_embedded".to_string();
        (use_socket, socket_dir, port, user)
    }

    pub fn logs(&self, tail: usize) -> Vec<String> {
        if let Ok(guard) = self.status.read() {
            if tail == 0 {
                return Vec::new();
            }
            let len = guard.logs.len();
            let start = len.saturating_sub(tail);
            return guard.logs[start..].to_vec();
        }
        Vec::new()
    }

    pub fn health_ok(&self) -> bool {
        self.status.read().map(|s| s.running).unwrap_or(false)
    }

    fn update_status<F>(&self, f: F)
    where
        F: FnOnce(&mut RuntimeStatus),
    {
        if let Ok(mut guard) = self.status.write() {
            f(&mut guard);
            self.emit_adapter_metrics(&guard);
        }
    }

    fn adapter_health_snapshot(&self) -> AdapterHealthState {
        self.status
            .read()
            .map(|s| AdapterHealthState::from(&*s))
            .unwrap_or_default()
    }

    fn emit_adapter_metrics(&self, status: &RuntimeStatus) {
        telemetry::set_counter("db.runtime.running", if status.running { 1 } else { 0 });
        telemetry::set_counter(
            "db.runtime.adapter.status_code",
            status.adapter_status.as_code(),
        );
        for variant in AdapterHealthStatus::variants() {
            let value = if status.adapter_status == variant {
                1
            } else {
                0
            };
            telemetry::set_counter(variant.metric_key(), value);
        }
    }

    fn record_probe(&self, latency_ms: f64, success: bool) {
        if let Some(diag) = self.diagnostics.get() {
            diag.record_probe("db-runtime", latency_ms, success);
        }
    }

    fn record_heartbeat(&self) {
        if let Some(diag) = self.diagnostics.get() {
            diag.record_heartbeat("db-runtime");
        }
    }

    fn runtime_root(&self) -> PathBuf {
        self.runtime_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.runtime_dir.clone())
    }

    /// Refresh the db_state.json snapshot on disk.
    /// Should be called before creating a backup to ensure the state is current.
    pub async fn refresh_state_snapshot(&self, checkpoint: Option<OffsetDateTime>) {
        let runtime_root = self.runtime_root();
        let adapter_health = self.adapter_health_snapshot();
        match collect_snapshot_with_health(&runtime_root, self.engine, checkpoint, adapter_health)
            .await
        {
            Ok(snapshot) => {
                if let Err(err) = persist_snapshot(&runtime_root, &snapshot).await {
                    warn!(error = %err, "failed to persist db runtime state snapshot");
                } else {
                    self.update_status(|s| {
                        s.last_checkpoint = snapshot.last_checkpoint;
                        s.applied_migrations = snapshot.applied_migrations.clone();
                        s.snapshot_updated_at = Some(snapshot.updated_at);
                        s.adapter_status = snapshot.adapter_health.status;
                    });
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to collect db runtime state snapshot");
            }
        }
    }

    async fn hydrate_state_from_disk(&self) {
        let runtime_root = self.runtime_root();
        match load_snapshot(&runtime_root).await {
            Ok(Some(snapshot)) => {
                let last_checkpoint = snapshot.last_checkpoint;
                let applied = snapshot.applied_migrations;
                let updated_at = snapshot.updated_at;
                let adapter_health = snapshot.adapter_health;
                self.update_status(move |s| {
                    s.last_checkpoint = last_checkpoint;
                    s.applied_migrations = applied;
                    s.snapshot_updated_at = Some(updated_at);
                    s.adapter_status = adapter_health.status;
                    if s.connector_uri.is_none() {
                        s.connector_uri = adapter_health.connector_uri.clone();
                    }
                    if s.port.is_none() {
                        s.port = adapter_health.port;
                    }
                    if s.pid.is_none() {
                        s.pid = adapter_health.pid;
                    }
                    if s.last_health.is_none() {
                        s.last_health = adapter_health.last_health;
                    }
                });
            }
            Ok(None) => {}
            Err(err) => warn!(error = %err, "failed to load db runtime state snapshot"),
        }
    }

    /// Create backup (without progress callback)
    pub async fn backup(&self, label: Option<String>) -> anyhow::Result<DbBackupArtifact> {
        self.backup_with_progress(label, None::<fn(BackupProgress)>)
            .await
    }

    /// Create backup with progress updates
    pub async fn backup_with_progress<F>(
        &self,
        label: Option<String>,
        progress: Option<F>,
    ) -> anyhow::Result<DbBackupArtifact>
    where
        F: Fn(BackupProgress) + Send + Sync,
    {
        let report = |p: BackupProgress| {
            if let Some(ref cb) = progress {
                cb(p);
            }
        };

        report(BackupProgress::Preparing);

        report(BackupProgress::CreatingBackup);
        let target = match self.engine {
            EmbeddedEngineKind::Sqlite => {
                let db_path = self.runtime_dir.join("fenrir.db");
                backup_sqlite(
                    &db_path,
                    &self.runtime_dir.join("backups"),
                    label.as_deref(),
                )?
            }
            EmbeddedEngineKind::Postgres => {
                let basebackup = self.detect_pg_basebackup();

                // Get connection info based on security settings
                let (use_socket, socket_dir, port, user) = self.connection_info();

                // Load credentials for password auth
                let password = if self.db_security.auth_method.requires_password() {
                    let creds = self.credentials().await?;
                    Some(creds.password)
                } else {
                    None
                };

                // Determine host - socket dir for Unix socket, localhost for TCP
                let host = if use_socket {
                    socket_dir.to_string_lossy().to_string()
                } else {
                    "127.0.0.1".to_string()
                };

                backup_postgres_with_auth(
                    &self.runtime_dir.join("postgres"),
                    &self.runtime_dir.join("backups"),
                    &basebackup,
                    &host,
                    port,
                    &user,
                    password.as_deref(),
                    label.as_deref(),
                )?
            }
        };
        report(BackupProgress::BackupCreated);

        report(BackupProgress::SavingState);
        let checkpoint = OffsetDateTime::now_utc();
        self.refresh_state_snapshot(Some(checkpoint)).await;
        let target_display = target.display().to_string();
        let target_display_for_log = target_display.clone();
        let state_manifest = match self.copy_snapshot_to_backup(&target).await {
            Ok(path) => Some(path.display().to_string()),
            Err(err) => {
                warn!(
                    error = %err,
                    backup = %target_display,
                    "failed to export db state for backup"
                );
                None
            }
        };
        report(BackupProgress::StateSaved);

        self.update_status(|s| {
            if let Some(ref manifest) = state_manifest {
                s.push_log(format!(
                    "backup created at {target_display_for_log} (state {manifest})"
                ));
            } else {
                s.push_log(format!("backup created at {target_display_for_log}"));
            }
            s.last_backup_path = Some(target_display_for_log.clone());
            s.last_backup_state_path = state_manifest.clone();
        });

        report(BackupProgress::Complete);
        Ok(DbBackupArtifact {
            artifact_path: target_display,
            state_snapshot_path: state_manifest,
        })
    }

    /// Restore database from backup (without progress callback)
    pub async fn restore(self: &Arc<Self>, backup_file: &str) -> anyhow::Result<()> {
        self.restore_with_progress(backup_file, None::<fn(RestoreProgress)>)
            .await
    }

    /// Restore database from backup with progress updates
    pub async fn restore_with_progress<F>(
        self: &Arc<Self>,
        backup_file: &str,
        progress: Option<F>,
    ) -> anyhow::Result<()>
    where
        F: Fn(RestoreProgress) + Send + Sync,
    {
        let report = |p: RestoreProgress| {
            if let Some(ref cb) = progress {
                cb(p);
            }
        };

        // For Postgres: stop the server before restore, then restart
        let needs_restart =
            self.engine == EmbeddedEngineKind::Postgres && self.running.load(Ordering::Acquire);

        if needs_restart {
            report(RestoreProgress::StoppingServer);
            info!("Stopping postgres for restore...");
            // Kill the postgres process
            let mut guard = self.pg_process.lock().await;
            if let Some(mut proc_info) = guard.take() {
                let _ = proc_info.child.kill();
                let _ = proc_info.child.wait();
            }
            drop(guard);

            // Clear the socket
            let socket_dir = self.runtime_dir.join("socket");
            let _ = fs::remove_dir_all(&socket_dir).await;
            report(RestoreProgress::ServerStopped);
        }

        // Perform the restore
        report(RestoreProgress::RestoringData);
        match self.engine {
            EmbeddedEngineKind::Sqlite => {
                restore_sqlite(Path::new(backup_file), &self.runtime_dir.join("fenrir.db"))?;
            }
            EmbeddedEngineKind::Postgres => {
                let data_dir = self.runtime_dir.join("postgres");
                restore_postgres(Path::new(backup_file), &data_dir)?;
                report(RestoreProgress::DataRestored);

                // Remove backup_label and recovery files so Postgres starts normally
                report(RestoreProgress::CleaningUp);
                let _ = std::fs::remove_file(data_dir.join("backup_label"));
                let _ = std::fs::remove_file(data_dir.join("backup_manifest"));
                let _ = std::fs::remove_file(data_dir.join("standby.signal"));
                let _ = std::fs::remove_file(data_dir.join("recovery.signal"));

                // Fix permissions - Postgres requires data dir to be 0700
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700));
                }

                info!("Cleaned up recovery files from backup");
                report(RestoreProgress::CleanupComplete);
            }
        }

        // Restore state snapshot
        let snapshot_restored = match self.restore_snapshot_from_backup(backup_file).await {
            Ok(source) => {
                self.update_status(|s| {
                    s.push_log(format!(
                        "restore from {backup_file} (state {})",
                        source.display()
                    ));
                });
                true
            }
            Err(err) => {
                warn!(
                    error = %err,
                    backup = %backup_file,
                    "restore completed but state snapshot missing"
                );
                self.update_status(|s| s.push_log(format!("restore from {}", backup_file)));
                false
            }
        };
        if snapshot_restored {
            self.hydrate_state_from_disk().await;
        }

        // Restart postgres after restore
        if needs_restart {
            report(RestoreProgress::StartingServer);
            info!("Restarting postgres after restore...");
            self.clone().start_embedded_postgres().await?;
            report(RestoreProgress::ServerStarted);
        }

        report(RestoreProgress::Complete);
        Ok(())
    }

    /// Copy the db_state.json snapshot to the backup directory.
    /// This is needed for restore to work properly.
    pub async fn copy_snapshot_to_backup(&self, backup_target: &Path) -> anyhow::Result<PathBuf> {
        let runtime_root = self.runtime_root();
        let snapshot_path = runtime_root.join("db_state.json");
        fs::metadata(&snapshot_path)
            .await
            .with_context(|| format!("db state snapshot missing at {}", snapshot_path.display()))?;
        let target_meta = fs::metadata(backup_target)
            .await
            .with_context(|| format!("inspect backup target {}", backup_target.display()))?;
        let resolved = if target_meta.is_dir() {
            backup_target.join("db_state.json")
        } else {
            let file_name = backup_target
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow::anyhow!("backup target missing file name"))?;
            let parent = backup_target
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            parent.join(format!("{file_name}.state.json"))
        };
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("prepare backup metadata dir {}", parent.display()))?;
        }
        fs::copy(&snapshot_path, &resolved).await.with_context(|| {
            format!(
                "copy snapshot {} to {}",
                snapshot_path.display(),
                resolved.display()
            )
        })?;
        Ok(resolved)
    }

    async fn restore_snapshot_from_backup(&self, backup_file: &str) -> anyhow::Result<PathBuf> {
        let backup_path = Path::new(backup_file);
        let meta = fs::metadata(backup_path)
            .await
            .with_context(|| format!("inspect backup {}", backup_path.display()))?;
        let source = if meta.is_dir() {
            backup_path.join("db_state.json")
        } else {
            PathBuf::from(format!("{backup_file}.state.json"))
        };
        fs::metadata(&source)
            .await
            .with_context(|| format!("state snapshot missing at {}", source.display()))?;
        let runtime_root = self.runtime_root();
        let dest = runtime_root.join("db_state.json");
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("prepare runtime dir {}", parent.display()))?;
        }
        fs::copy(&source, &dest)
            .await
            .with_context(|| format!("copy snapshot {} to {}", source.display(), dest.display()))?;
        Ok(source)
    }
}

#[async_trait::async_trait]
impl ManagedService for DbRuntimeSupervisor {
    fn id(&self) -> &'static str {
        "db-runtime"
    }

    async fn start(self: Arc<Self>) -> anyhow::Result<bool> {
        self.hydrate_state_from_disk().await;
        let was_running = self.running.swap(true, Ordering::AcqRel);
        if !was_running {
            if let Err(err) = fs::create_dir_all(&self.runtime_dir).await {
                self.running.store(false, Ordering::Release);
                return Err(anyhow::anyhow!(err).context("prepare db runtime dir"));
            }
            match self.engine {
                EmbeddedEngineKind::Sqlite => {
                    let db_path = self.runtime_dir.join("fenrir.db");
                    // Touch the file; sqlite will create if missing.
                    if let Err(err) = fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .open(&db_path)
                        .await
                    {
                        warn!(error = %err, "failed to prepare embedded sqlite file");
                    } else {
                        let uri = format!("sqlite://{}", db_path.display());
                        self.set_connector_uri(uri);
                        // Seal or load credentials for embedded access (if security manager attached).
                        if let Some(security) = self.security.get() {
                            if let Err(err) =
                                SealedDbCredentials::ensure(security, &self.runtime_dir).await
                            {
                                warn!(error = %err, "failed to seal embedded db credentials");
                            }
                        }
                    }
                }
                EmbeddedEngineKind::Postgres => {
                    if let Err(err) = self.start_embedded_postgres().await {
                        self.running.store(false, Ordering::Release);
                        self.update_status(|s| {
                            s.running = false;
                            s.adapter_status = AdapterHealthStatus::Failed;
                            s.push_log(format!("postgres start failed: {err}"));
                        });
                        return Err(err);
                    }
                }
            }
            info!(
                mode = ?self.mode,
                engine = ?self.engine,
                dir = %self.runtime_dir.display(),
                "db runtime supervisor started (stub)"
            );
            self.update_status(|s| {
                s.running = true;
                s.adapter_status = AdapterHealthStatus::Ready;
                s.last_health = Some(OffsetDateTime::now_utc());
                s.push_log("db-runtime started");
            });
            self.refresh_state_snapshot(None).await;
            self.record_probe(0.0, true);
            self.record_heartbeat();
        }
        Ok(!was_running)
    }

    async fn stop(self: Arc<Self>, _force: bool) -> anyhow::Result<bool> {
        let was_running = self.running.swap(false, Ordering::AcqRel);
        if was_running {
            info!(
                mode = ?self.mode,
                engine = ?self.engine,
                dir = %self.runtime_dir.display(),
                "db runtime supervisor stopped (stub)"
            );
            self.update_status(|s| {
                s.running = false;
                s.adapter_status = AdapterHealthStatus::Stopped;
                s.push_log("db-runtime stopped");
            });
            self.record_probe(0.0, false);
            if self.engine == EmbeddedEngineKind::Postgres {
                let mut guard = self.pg_process.lock().await;
                if let Some(proc_info) = guard.as_mut() {
                    let _ = proc_info.child.kill();
                    let _ = proc_info.child.wait();
                    self.update_status(|s| {
                        s.push_log("postgres process terminated");
                        s.connector_uri = None;
                        s.port = None;
                        s.pid = None;
                    });
                }
                *guard = None;
            }
        }
        if was_running {
            self.refresh_state_snapshot(Some(OffsetDateTime::now_utc()))
                .await;
        }
        Ok(was_running)
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeStatus {
    pub engine: EmbeddedEngineKind,
    pub running: bool,
    pub connector_uri: Option<String>,
    pub last_health: Option<OffsetDateTime>,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub last_checkpoint: Option<OffsetDateTime>,
    pub applied_migrations: Vec<String>,
    pub logs: Vec<String>,
    pub adapter_status: AdapterHealthStatus,
    pub snapshot_updated_at: Option<OffsetDateTime>,
    pub last_backup_path: Option<String>,
    pub last_backup_state_path: Option<String>,
}

impl RuntimeStatus {
    fn default(engine: EmbeddedEngineKind) -> Self {
        Self {
            engine,
            running: false,
            connector_uri: None,
            last_health: None,
            port: None,
            pid: None,
            last_checkpoint: None,
            applied_migrations: Vec::new(),
            logs: Vec::new(),
            adapter_status: AdapterHealthStatus::Unknown,
            snapshot_updated_at: None,
            last_backup_path: None,
            last_backup_state_path: None,
        }
    }

    fn push_log(&mut self, msg: impl Into<String>) {
        const MAX_LOGS: usize = 64;
        self.logs.push(msg.into());
        if self.logs.len() > MAX_LOGS {
            let drop = self.logs.len() - MAX_LOGS;
            self.logs.drain(0..drop);
        }
    }
}

impl From<&RuntimeStatus> for AdapterHealthState {
    fn from(status: &RuntimeStatus) -> Self {
        AdapterHealthState {
            status: status.adapter_status,
            running: status.running,
            last_health: status.last_health,
            connector_uri: status.connector_uri.clone(),
            port: status.port,
            pid: status.pid,
        }
    }
}

struct PostgresProcess {
    child: Child,
    _port: u16,
}

impl DbRuntimeSupervisor {
    async fn start_embedded_postgres(self: &Arc<Self>) -> anyhow::Result<()> {
        let data_dir = self.runtime_dir.join("postgres");
        let socket_dir = self.runtime_dir.join("socket");
        fs::create_dir_all(&data_dir).await?;
        fs::create_dir_all(&socket_dir).await?;

        // Load or create credentials (if using password auth)
        let credentials = if self.db_security.auth_method.requires_password() {
            let security = self
                .security
                .get()
                .ok_or_else(|| anyhow!("SecurityManager required for password-based DB auth"))?;
            Some(SealedDbCredentials::load_or_create(security, &self.runtime_dir).await?)
        } else {
            None
        };

        // initdb if needed
        if !data_dir.join("PG_VERSION").exists() {
            self.run_initdb(&data_dir, credentials.as_ref()).await?;
        }

        // Find a free port (still needed for TCP fallback and health checks)
        let port = find_free_port(self.port_range).ok_or_else(|| {
            anyhow!(
                "no free port in range {}-{}",
                self.port_range.min,
                self.port_range.max
            )
        })?;

        // Start postgres with secure settings
        let pg_bin = self.detect_postgres_binary();
        let mut cmd = Command::new(pg_bin);
        cmd.arg("-D").arg(&data_dir);

        if self.db_security.prefer_unix_socket {
            // Unix socket mode: more secure, no network exposure
            cmd.arg("-k")
                .arg(&socket_dir) // Socket directory
                .arg("-h")
                .arg(""); // Empty = disable TCP completely
            info!(socket_dir = %socket_dir.display(), "postgres using Unix socket (TCP disabled)");
        } else {
            // TCP mode: localhost only with password
            cmd.arg("-p")
                .arg(port.to_string())
                .arg("-h")
                .arg("127.0.0.1");
        }

        // Fix macOS issues
        cmd.env("LC_ALL", "C")
            .env("LANG", "C")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| anyhow!(e))?;
        let pid = child.id();
        self.consume_logs("postgres", child.stdout.take(), child.stderr.take());
        {
            let mut guard = self.pg_process.lock().await;
            *guard = Some(PostgresProcess { child, _port: port });
        }

        // Build the connection URI before probing, so the probe exercises the
        // exact path the migration runner and the connector will use.
        let uri = self.build_connection_uri(port, &socket_dir, credentials.as_ref());

        // Health probe - wait until postgres actually serves queries.
        //
        // A socket- or TCP-level connect is NOT a readiness signal: postgres
        // begins listening before startup/recovery completes and answers
        // `FATAL: the database system is starting up` to whoever connects in
        // that window. Probing at transport level therefore reported "ready"
        // while the very next step - applying migrations - still failed.
        if self.db_security.prefer_unix_socket {
            let socket_path = socket_dir.join(".s.PGSQL.5432");
            Self::await_socket_file(&socket_path).await?;
        } else {
            Self::await_tcp_listener(port).await?;
        }
        Self::await_postgres_handshake(&uri).await?;

        self.set_connector_uri(uri.clone());

        let connection_mode = if self.db_security.prefer_unix_socket {
            format!("socket:{}", socket_dir.display())
        } else {
            format!("tcp:127.0.0.1:{port}")
        };
        let auth_mode = self.db_security.auth_method.as_pg_arg();

        self.update_status(|s| {
            s.running = true;
            s.port = Some(port);
            s.pid = Some(pid);
            s.last_health = Some(OffsetDateTime::now_utc());
            s.push_log(format!(
                "postgres started ({connection_mode}, auth={auth_mode}, pid={pid})"
            ));
        });

        info!(
            port = port,
            pid = pid,
            auth = auth_mode,
            socket = self.db_security.prefer_unix_socket,
            "embedded postgres started securely"
        );

        Ok(())
    }

    /// Run initdb with appropriate authentication settings
    async fn run_initdb(
        self: &Arc<Self>,
        data_dir: &Path,
        credentials: Option<&DbCredentials>,
    ) -> anyhow::Result<()> {
        let initdb = self.detect_initdb_binary();
        let mut cmd = Command::new(initdb);
        cmd.arg("-D")
            .arg(data_dir)
            .arg("--username=fenrir_embedded")
            .arg(format!(
                "--auth={}",
                self.db_security.auth_method.as_pg_arg()
            ))
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // If using password auth, write password to temp file
        let pw_file = if let Some(creds) = credentials {
            let pw_path = self.runtime_dir.join(".pg_init_pw");
            fs::write(&pw_path, &creds.password).await?;
            cmd.arg("--pwfile").arg(&pw_path);
            Some(pw_path)
        } else {
            None
        };

        let mut child = cmd.spawn().map_err(|e| anyhow!(e))?;
        self.consume_logs("initdb", child.stdout.take(), child.stderr.take());
        let status = child.wait().map_err(|e| anyhow!(e))?;

        // Clean up password file
        if let Some(pw_path) = pw_file {
            let _ = fs::remove_file(&pw_path).await;
        }

        if !status.success() {
            return Err(anyhow!("initdb failed with {:?}", status.code()));
        }

        self.update_status(|s| {
            s.push_log(format!(
                "initdb completed (auth={})",
                self.db_security.auth_method.as_pg_arg()
            ))
        });

        Ok(())
    }

    /// Build the connection URI based on security settings
    /// Waits for postgres to create its Unix socket file.
    ///
    /// Only the file's existence - the handshake probe that follows is what
    /// establishes readiness. Split out so the failure message names the
    /// actual stage that timed out.
    async fn await_socket_file(socket_path: &Path) -> anyhow::Result<()> {
        let started = Instant::now();
        while started.elapsed() < PG_SOCKET_WAIT {
            if socket_path.exists() {
                return Ok(());
            }
            tokio::time::sleep(PG_PROBE_INTERVAL).await;
        }
        Err(anyhow!(
            "postgres did not create its socket at {} within {:?}",
            socket_path.display(),
            PG_SOCKET_WAIT
        ))
    }

    /// Waits for postgres to start listening on the loopback port.
    async fn await_tcp_listener(port: u16) -> anyhow::Result<()> {
        let addr = format!("127.0.0.1:{port}");
        let started = Instant::now();
        while started.elapsed() < PG_SOCKET_WAIT {
            if TcpStream::connect(&addr).is_ok() {
                return Ok(());
            }
            tokio::time::sleep(PG_PROBE_INTERVAL).await;
        }
        Err(anyhow!(
            "postgres did not listen on {addr} within {:?}",
            PG_SOCKET_WAIT
        ))
    }

    /// Polls until postgres completes a real handshake and answers a query.
    ///
    /// This is the readiness definition that matters: it is exactly what the
    /// migration runner does next. Errors while starting up are expected and
    /// retried; the last one is reported if the budget runs out.
    async fn await_postgres_handshake(uri: &str) -> anyhow::Result<()> {
        let started = Instant::now();
        let mut last_error: Option<String> = None;

        while started.elapsed() < PG_HANDSHAKE_WAIT {
            match tokio_postgres::connect(uri, tokio_postgres::NoTls).await {
                Ok((client, connection)) => {
                    // The connection future drives the socket; it has to run
                    // for the query to resolve, and is dropped right after.
                    let driver = tokio::spawn(async move {
                        let _ = connection.await;
                    });
                    let probed = client.simple_query("SELECT 1").await;
                    drop(client);
                    driver.abort();

                    match probed {
                        Ok(_) => {
                            info!(
                                waited_ms = started.elapsed().as_millis() as u64,
                                "postgres accepting connections"
                            );
                            return Ok(());
                        }
                        Err(err) => last_error = Some(err.to_string()),
                    }
                }
                Err(err) => last_error = Some(err.to_string()),
            }
            tokio::time::sleep(PG_PROBE_INTERVAL).await;
        }

        Err(anyhow!(
            "postgres did not accept connections within {:?}: {}",
            PG_HANDSHAKE_WAIT,
            last_error.unwrap_or_else(|| "no error reported".to_string())
        ))
    }

    fn build_connection_uri(
        &self,
        port: u16,
        socket_dir: &Path,
        credentials: Option<&DbCredentials>,
    ) -> String {
        let user = credentials
            .map(|c| c.user.as_str())
            .unwrap_or("fenrir_embedded");

        if self.db_security.prefer_unix_socket {
            // Unix socket connection (most secure)
            if let Some(creds) = credentials {
                // URL-encode password for safety
                let encoded_pw = urlencoding::encode(&creds.password);
                format!(
                    "postgres://{}:{}@/postgres?host={}",
                    user,
                    encoded_pw,
                    socket_dir.display()
                )
            } else {
                format!(
                    "postgres://{}@/postgres?host={}",
                    user,
                    socket_dir.display()
                )
            }
        } else {
            // TCP connection (localhost only)
            if let Some(creds) = credentials {
                let encoded_pw = urlencoding::encode(&creds.password);
                format!(
                    "postgres://{}:{}@127.0.0.1:{}/postgres",
                    user, encoded_pw, port
                )
            } else {
                format!("postgres://{}@127.0.0.1:{}/postgres", user, port)
            }
        }
    }

    fn detect_initdb_binary(&self) -> PathBuf {
        let pg_bin = self.detect_postgres_binary();
        if let Some(parent) = pg_bin.parent() {
            let candidate = parent.join("initdb");
            if candidate.exists() {
                return candidate;
            }
        }
        PathBuf::from("initdb")
    }

    fn detect_postgres_binary(&self) -> PathBuf {
        let configured = PathBuf::from(&self.pg_binary);
        if configured.exists() {
            configured
        } else {
            PathBuf::from("postgres")
        }
    }

    fn detect_pg_basebackup(&self) -> PathBuf {
        let configured = PathBuf::from(&self.pg_binary);
        if let Some(parent) = configured.parent() {
            let candidate = parent.join("pg_basebackup");
            if candidate.exists() {
                return candidate;
            }
        }
        PathBuf::from("pg_basebackup")
    }

    fn consume_logs(
        self: &Arc<Self>,
        label: &str,
        stdout: Option<std::process::ChildStdout>,
        stderr: Option<std::process::ChildStderr>,
    ) {
        let label_out = format!("{label}:stdout");
        let label_err = format!("{label}:stderr");
        if let Some(out) = stdout {
            let status = Arc::clone(self);
            thread::spawn(move || {
                let reader = BufReader::new(out);
                for text in reader.lines().map_while(Result::ok) {
                    status.update_status(|s| s.push_log(format!("{label_out} {text}")));
                }
            });
        }
        if let Some(err) = stderr {
            let status = Arc::clone(self);
            thread::spawn(move || {
                let reader = BufReader::new(err);
                for text in reader.lines().map_while(Result::ok) {
                    status.update_status(|s| s.push_log(format!("{label_err} {text}")));
                }
            });
        }
    }
}

fn find_free_port(range: DbEmbeddedPortRange) -> Option<u16> {
    for port in range.min..=range.max {
        let addr = format!("127.0.0.1:{port}");
        if TcpStream::connect(&addr).is_err() {
            return Some(port);
        }
    }
    None
}
