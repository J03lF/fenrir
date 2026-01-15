use std::collections::{BTreeMap, HashSet};
use std::fs as std_fs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json;
use tokio::fs;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use super::types::{JobControlOutcome, ScheduledJobSnapshot, ScheduledJobSpec, SchedulerError};
use crate::audit::{AuditActor, AuditEvent, AuditMetadata, AuditOutcome};
use crate::infra::telemetry;
use crate::services::{
    AppServices, DbShellService, ModuleService, ServiceDiagnostics, ServiceRegistry, ServiceStatus,
    TokenExchangeService,
};
use crate::utils::messages::services::scheduler::{
    debug as scheduler_debug, descriptions as scheduler_descriptions, errors as scheduler_errors,
    notes as scheduler_notes, service as scheduler_service_messages,
};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub struct SchedulerService {
    registry: Arc<ServiceRegistry>,
    diagnostics: Arc<ServiceDiagnostics>,
    heartbeat: Mutex<Option<JoinHandle<()>>>,
    jobs: Mutex<BTreeMap<String, JobEntry>>,
    paused_jobs: Mutex<HashSet<String>>,
    state_path: PathBuf,
}

struct JobEntry {
    spec: ScheduledJobSpec,
    task: JobTaskHandle,
    handle: Option<JobHandle>,
    paused: bool,
}

impl JobEntry {
    fn snapshot(&self, scheduler_active: bool) -> ScheduledJobSnapshot {
        ScheduledJobSnapshot {
            id: self.spec.id.clone(),
            interval: self.spec.interval,
            description: self.spec.description.clone(),
            active: scheduler_active && self.handle.is_some(),
            paused: self.paused,
        }
    }
}

struct JobHandle {
    stop_flag: Arc<AtomicBool>,
    notifier: Arc<Notify>,
    join: JoinHandle<()>,
}

impl JobHandle {
    fn stop(self) {
        self.stop_flag.store(true, Ordering::Release);
        self.notifier.notify_waiters();
        self.join.abort();
    }
}

const TOKEN_REFRESH_THRESHOLD_SECS: i64 = 180;
const AUDIT_DRAIN_SAMPLE_LIMIT: usize = 512;

type JobTaskHandle = Arc<dyn JobTask>;

trait JobTask: Send + Sync {
    fn invoke(&self) -> BoxFuture<'static, anyhow::Result<()>>;
}

impl<F, Fut> JobTask for F
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    fn invoke(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin((self)())
    }
}

impl SchedulerService {
    pub fn new(
        registry: Arc<ServiceRegistry>,
        diagnostics: Arc<ServiceDiagnostics>,
        state_dir: PathBuf,
    ) -> Self {
        let state_path = state_dir.join("scheduler_state.json");
        if let Some(parent) = state_path.parent() {
            let _ = std_fs::create_dir_all(parent);
        }
        let paused = load_paused_jobs(&state_path);
        Self {
            registry,
            diagnostics,
            heartbeat: Mutex::new(None),
            jobs: Mutex::new(BTreeMap::new()),
            paused_jobs: Mutex::new(paused),
            state_path,
        }
    }

    pub fn start(&self) -> bool {
        {
            let guard = self.heartbeat.lock().expect("scheduler heartbeat lock");
            if guard.is_some() {
                info!("{}", scheduler_service_messages::ALREADY_RUNNING);
                return false;
            }
        }
        self.registry.set_status(
            "scheduler",
            ServiceStatus::Starting,
            Some(scheduler_service_messages::STARTING_NOTE.to_string()),
        );
        let registry = Arc::clone(&self.registry);
        let diagnostics = Arc::clone(&self.diagnostics);
        let handle = tokio::spawn(async move {
            registry.set_status(
                "scheduler",
                ServiceStatus::Active,
                Some(scheduler_service_messages::HEARTBEAT_ACTIVE_NOTE.to_string()),
            );
            diagnostics.record_heartbeat("scheduler");
            info!("{}", scheduler_service_messages::HEARTBEAT_LOOP_STARTED);

            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                registry.update_note(
                    "scheduler",
                    Some(scheduler_service_messages::HEARTBEAT_OK_NOTE.to_string()),
                );
                diagnostics.record_heartbeat("scheduler");
                debug!(
                    target: "scheduler",
                    "{}",
                    scheduler_debug::HEARTBEAT_SENT
                );
            }
        });
        let mut guard = self.heartbeat.lock().expect("scheduler heartbeat lock");
        *guard = Some(handle);
        true
    }

    pub fn stop(&self) -> bool {
        let mut heartbeat = self.heartbeat.lock().expect("scheduler heartbeat lock");
        let Some(handle) = heartbeat.take() else {
            debug!("{}", scheduler_service_messages::STOP_REQUEST_IGNORED);
            return false;
        };
        handle.abort();
        drop(heartbeat);

        self.clear_jobs();
        self.registry.set_status(
            "scheduler",
            ServiceStatus::Standby,
            Some(scheduler_service_messages::STOPPED_NOTE.to_string()),
        );
        info!("{}", scheduler_service_messages::HEARTBEAT_STOPPED);
        true
    }

    pub fn is_running(&self) -> bool {
        self.heartbeat
            .lock()
            .expect("scheduler heartbeat lock")
            .is_some()
    }

    pub fn clear_jobs(&self) {
        let mut guard = self.jobs.lock().expect("scheduler jobs lock");
        while let Some((id, mut entry)) = guard.pop_first() {
            if let Some(handle) = entry.handle.take() {
                handle.stop();
            }
            debug!(job = %id, "{}", scheduler_debug::JOB_CLEARED);
        }
        self.registry.update_note(
            "scheduler",
            Some(scheduler_service_messages::JOBS_INACTIVE_NOTE.to_string()),
        );
    }

    pub fn schedule_fixed_rate<F, Fut>(
        &self,
        spec: ScheduledJobSpec,
        job: F,
    ) -> Result<(), SchedulerError>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let task: JobTaskHandle = Arc::new(job);
        self.register_job(spec, task, false)
    }

    pub fn cancel_job(&self, id: &str) -> bool {
        let mut guard = self.jobs.lock().expect("scheduler jobs lock");
        if let Some(mut entry) = guard.remove(id) {
            if let Some(handle) = entry.handle.take() {
                handle.stop();
            }
            self.registry
                .update_note("scheduler", Some(scheduler_notes::job_stopped(id)));
            true
        } else {
            false
        }
    }

    pub fn jobs(&self) -> Vec<ScheduledJobSnapshot> {
        let scheduler_active = self.is_running();
        self.jobs
            .lock()
            .expect("scheduler jobs lock")
            .values()
            .map(|entry| entry.snapshot(scheduler_active))
            .collect()
    }

    pub fn job(&self, id: &str) -> Option<ScheduledJobSnapshot> {
        let scheduler_active = self.is_running();
        self.jobs
            .lock()
            .ok()
            .and_then(|guard| guard.get(id).map(|entry| entry.snapshot(scheduler_active)))
    }

    pub fn restart_job(&self, id: &str) -> Result<JobControlOutcome, SchedulerError> {
        if !self.is_running() {
            return Ok(JobControlOutcome::SchedulerInactive);
        }

        let mut guard = self.jobs.lock().expect("scheduler jobs lock");
        let Some(entry) = guard.get_mut(id) else {
            return Err(SchedulerError::JobNotFound(id.to_string()));
        };

        if entry.paused {
            return Ok(JobControlOutcome::Paused);
        }

        if let Some(handle) = entry.handle.take() {
            handle.stop();
        }

        let registry = Arc::clone(&self.registry);
        let handle = Self::spawn_job(&entry.spec, Arc::clone(&entry.task), registry, true);
        entry.handle = Some(handle);
        Ok(JobControlOutcome::Restarted)
    }

    pub fn pause_job(&self, id: &str) -> Result<JobControlOutcome, SchedulerError> {
        let mut guard = self.jobs.lock().expect("scheduler jobs lock");
        let Some(entry) = guard.get_mut(id) else {
            return Err(SchedulerError::JobNotFound(id.to_string()));
        };

        if entry.paused {
            return Ok(JobControlOutcome::AlreadyPaused);
        }

        if let Some(handle) = entry.handle.take() {
            handle.stop();
        }
        entry.paused = true;

        let paused_snapshot = {
            let mut paused = self.paused_jobs.lock().expect("scheduler paused jobs lock");
            paused.insert(id.to_string());
            paused.iter().cloned().collect::<Vec<_>>()
        };
        self.persist_paused_jobs(paused_snapshot);
        self.registry
            .update_note("scheduler", Some(scheduler_notes::job_paused(id)));
        Ok(JobControlOutcome::Paused)
    }

    pub fn resume_job(&self, id: &str) -> Result<JobControlOutcome, SchedulerError> {
        if !self.is_running() {
            return Ok(JobControlOutcome::SchedulerInactive);
        }
        let mut guard = self.jobs.lock().expect("scheduler jobs lock");
        let Some(entry) = guard.get_mut(id) else {
            return Err(SchedulerError::JobNotFound(id.to_string()));
        };

        if !entry.paused {
            return Ok(JobControlOutcome::AlreadyActive);
        }
        entry.paused = false;

        let paused_snapshot = {
            let mut paused = self.paused_jobs.lock().expect("scheduler paused jobs lock");
            paused.remove(id);
            paused.iter().cloned().collect::<Vec<_>>()
        };
        self.persist_paused_jobs(paused_snapshot);

        if let Some(handle) = entry.handle.take() {
            handle.stop();
        }
        let registry = Arc::clone(&self.registry);
        let handle = Self::spawn_job(&entry.spec, Arc::clone(&entry.task), registry, true);
        entry.handle = Some(handle);
        self.registry
            .update_note("scheduler", Some(scheduler_notes::job_resumed(id)));
        Ok(JobControlOutcome::Resumed)
    }

    fn register_job(
        &self,
        spec: ScheduledJobSpec,
        task: JobTaskHandle,
        skip_initial_delay: bool,
    ) -> Result<(), SchedulerError> {
        if spec.interval.is_zero() {
            return Err(SchedulerError::InvalidInterval);
        }
        if self
            .heartbeat
            .lock()
            .expect("scheduler heartbeat lock")
            .is_none()
        {
            return Err(SchedulerError::SchedulerNotStarted);
        }

        let job_id = spec.id.clone();
        let mut guard = self.jobs.lock().expect("scheduler jobs lock");
        if guard.contains_key(&job_id) {
            return Err(SchedulerError::JobAlreadyExists(job_id));
        }

        let registry = Arc::clone(&self.registry);
        let interval = spec.interval;
        let is_paused = {
            let paused = self.paused_jobs.lock().expect("scheduler paused jobs lock");
            paused.contains(&job_id)
        };
        let handle = if is_paused {
            None
        } else {
            Some(Self::spawn_job(
                &spec,
                Arc::clone(&task),
                registry,
                skip_initial_delay,
            ))
        };

        guard.insert(
            job_id.clone(),
            JobEntry {
                spec,
                task,
                handle,
                paused: is_paused,
            },
        );
        self.registry.update_note(
            "scheduler",
            Some(if is_paused {
                scheduler_notes::job_paused(&job_id)
            } else {
                scheduler_notes::job_active(&job_id, interval.as_secs())
            }),
        );
        Ok(())
    }

    fn spawn_job(
        spec: &ScheduledJobSpec,
        task: JobTaskHandle,
        registry: Arc<ServiceRegistry>,
        skip_initial_delay: bool,
    ) -> JobHandle {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let notifier = Arc::new(Notify::new());
        let job_id = spec.id.clone();
        let interval = spec.interval;
        let initial_delay = if skip_initial_delay {
            None
        } else {
            spec.initial_delay
        };
        let stop_clone = Arc::clone(&stop_flag);
        let notifier_clone = Arc::clone(&notifier);
        let task_runner = Arc::clone(&task);
        let registry_for_errors = Arc::clone(&registry);

        let join = tokio::spawn(async move {
            if let Some(delay) = initial_delay {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {},
                    _ = notifier_clone.notified() => return,
                }
            }

            loop {
                if stop_clone.load(Ordering::Acquire) {
                    break;
                }
                if let Err(err) = task_runner.invoke().await {
                    error!(
                        job = %job_id,
                        error = %err,
                        "{}",
                        scheduler_errors::JOB_FAILED
                    );
                    registry_for_errors.update_note(
                        "scheduler",
                        Some(scheduler_notes::job_failure(&job_id, &err)),
                    );
                }
                tokio::select! {
                    _ = notifier_clone.notified() => break,
                    _ = tokio::time::sleep(interval) => {}
                }
            }
        });

        JobHandle {
            stop_flag,
            notifier,
            join,
        }
    }

    fn persist_paused_jobs(&self, mut paused_ids: Vec<String>) {
        paused_ids.sort();
        let state = SchedulerStateFile { paused: paused_ids };
        match serde_json::to_vec(&state)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| std_fs::write(&self.state_path, bytes).map_err(anyhow::Error::from))
        {
            Ok(_) => {}
            Err(err) => warn!(
                path = %self.state_path.display(),
                error = %err,
                "failed to persist scheduler paused state"
            ),
        }
    }
}
pub struct SchedulerJobContext {
    pub registry: Arc<ServiceRegistry>,
    pub db_shell: Arc<DbShellService>,
    pub diagnostics: Arc<ServiceDiagnostics>,
    pub module_service: Arc<ModuleService>,
    pub services: Arc<AppServices>,
    pub runtime_dir: PathBuf,
    pub token_exchange: Arc<TokenExchangeService>,
    pub backup_service: Option<Arc<crate::services::backup::BackupService>>,
}

pub fn install_default_jobs(
    scheduler: &SchedulerService,
    ctx: SchedulerJobContext,
) -> Result<(), SchedulerError> {
    let SchedulerJobContext {
        registry,
        db_shell,
        diagnostics,
        module_service,
        services,
        runtime_dir,
        token_exchange,
        backup_service,
    } = ctx;
    let registry_for_uptime = Arc::clone(&registry);
    scheduler.schedule_fixed_rate(
        ScheduledJobSpec {
            id: "telemetry-health-refresh".to_string(),
            interval: Duration::from_secs(60),
            initial_delay: Some(Duration::from_secs(10)),
            description: scheduler_descriptions::TELEMETRY_HEALTH_REFRESH.to_string(),
        },
        move || {
            let registry = Arc::clone(&registry_for_uptime);
            async move {
                if let Some(snapshot) = telemetry::snapshot() {
                    registry.update_note(
                        "scheduler",
                        Some(scheduler_notes::uptime(snapshot.uptime.as_secs())),
                    );
                }
                Ok::<(), anyhow::Error>(())
            }
        },
    )?;

    let registry_for_health = Arc::clone(&registry);
    scheduler.schedule_fixed_rate(
        ScheduledJobSpec {
            id: "service-health-scan".to_string(),
            interval: Duration::from_secs(30),
            initial_delay: Some(Duration::from_secs(5)),
            description: scheduler_descriptions::SERVICE_HEALTH_SCAN.to_string(),
        },
        move || {
            let registry = Arc::clone(&registry_for_health);
            async move {
                let snapshot = registry.snapshot();
                let failed = snapshot
                    .iter()
                    .filter(|svc| matches!(svc.status, ServiceStatus::Failed))
                    .count();
                let degraded = snapshot
                    .iter()
                    .filter(|svc| matches!(svc.status, ServiceStatus::Degraded))
                    .count();
                registry.update_note(
                    "scheduler",
                    Some(scheduler_notes::job_health(failed, degraded)),
                );
                Ok::<(), anyhow::Error>(())
            }
        },
    )?;

    let registry_for_db = Arc::clone(&registry);
    let diagnostics_for_db = Arc::clone(&diagnostics);
    scheduler.schedule_fixed_rate(
        ScheduledJobSpec {
            id: "db-default-ping".to_string(),
            interval: Duration::from_secs(120),
            initial_delay: Some(Duration::from_secs(15)),
            description: scheduler_descriptions::DB_DEFAULT_PING.to_string(),
        },
        move || {
            let registry = Arc::clone(&registry_for_db);
            let db_shell = Arc::clone(&db_shell);
            let diagnostics = Arc::clone(&diagnostics_for_db);
            async move {
                let session = db_shell.create_session();
                let started_at = Instant::now();
                match session.ping().await {
                    Ok(_) => {
                        diagnostics.record_probe(
                            "db-shell",
                            started_at.elapsed().as_secs_f64() * 1000.0,
                            true,
                        );
                        registry.set_status(
                            "db-shell",
                            ServiceStatus::Active,
                            Some(scheduler_service_messages::DB_AVAILABLE_NOTE.to_string()),
                        );
                    }
                    Err(err) => {
                        diagnostics.record_probe(
                            "db-shell",
                            started_at.elapsed().as_secs_f64() * 1000.0,
                            false,
                        );
                        warn!(error = %err, "{}", scheduler_service_messages::DB_PING_FAILED);
                        registry.set_status(
                            "db-shell",
                            ServiceStatus::Degraded,
                            Some(scheduler_notes::db_unreachable(&err)),
                        );
                    }
                }
                Ok::<(), anyhow::Error>(())
            }
        },
    )?;

    let module_service_for_tokens = Arc::clone(&module_service);
    let services_for_tokens = Arc::clone(&services);
    let token_exchange_for_tokens = Arc::clone(&token_exchange);
    scheduler.schedule_fixed_rate(
        ScheduledJobSpec {
            id: "token-lease-monitor".to_string(),
            interval: Duration::from_secs(60),
            initial_delay: Some(Duration::from_secs(20)),
            description: scheduler_descriptions::TOKEN_LEASE_MONITOR.to_string(),
        },
        move || {
            let module_service = Arc::clone(&module_service_for_tokens);
            let services = Arc::clone(&services_for_tokens);
            let token_exchange = Arc::clone(&token_exchange_for_tokens);
            async move {
                let leases = module_service.token_leases_snapshot().await;
                for (module_id, lease) in leases {
                    if lease.seconds_until_expiry() <= TOKEN_REFRESH_THRESHOLD_SECS {
                        let module_id_str = module_id.to_string();
                        match token_exchange.force_refresh(&module_id).await {
                            Ok(new_lease) => {
                                record_token_refresh_audit(
                                    &services,
                                    &module_id_str,
                                    new_lease.seconds_until_expiry(),
                                    AuditOutcome::Success,
                                    None,
                                );
                            }
                            Err(err) => {
                                tracing::warn!(
                                    module = %module_id_str,
                                    error = %err,
                                    "token lease refresh failed"
                                );
                                record_token_refresh_audit(
                                    &services,
                                    &module_id_str,
                                    lease.seconds_until_expiry(),
                                    AuditOutcome::Failure,
                                    Some(err.to_string()),
                                );
                            }
                        }
                    }
                }
                Ok::<(), anyhow::Error>(())
            }
        },
    )?;

    let module_service_for_health = Arc::clone(&module_service);
    scheduler.schedule_fixed_rate(
        ScheduledJobSpec {
            id: "module-heartbeat-verifier".to_string(),
            interval: Duration::from_secs(45),
            initial_delay: Some(Duration::from_secs(15)),
            description: scheduler_descriptions::MODULE_HEARTBEAT_VERIFIER.to_string(),
        },
        move || {
            let module_service = Arc::clone(&module_service_for_health);
            async move {
                if let Err(err) = module_service.verify_runtime_health().await {
                    tracing::warn!(
                        error = %err,
                        "module heartbeat verification failed"
                    );
                }
                Ok::<(), anyhow::Error>(())
            }
        },
    )?;

    let services_for_audit = Arc::clone(&services);
    let audit_drain_dir = runtime_dir.join("audit").join("drain");
    scheduler.schedule_fixed_rate(
        ScheduledJobSpec {
            id: "audit-drain".to_string(),
            interval: Duration::from_secs(300),
            initial_delay: Some(Duration::from_secs(60)),
            description: scheduler_descriptions::AUDIT_DRAIN.to_string(),
        },
        move || {
            let services = Arc::clone(&services_for_audit);
            let drain_dir = audit_drain_dir.clone();
            async move {
                match services.audit_recent(AUDIT_DRAIN_SAMPLE_LIMIT) {
                    Ok(events) if !events.is_empty() => {
                        if let Err(err) = write_audit_snapshot(&drain_dir, &events).await {
                            tracing::warn!(
                                error = %err,
                                "audit drain job failed to persist snapshot"
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "audit drain job failed to read audit events"
                        );
                    }
                }
                Ok::<(), anyhow::Error>(())
            }
        },
    )?;

    // Automatic database backup job (if enabled)
    if let Some(backup_svc) = backup_service {
        if backup_svc.is_enabled() {
            let backup_interval = parse_backup_schedule(backup_svc.config().schedule.as_str());
            let backup_service_for_job = Arc::clone(&backup_svc);
            let registry_for_backup = Arc::clone(&registry);

            scheduler.schedule_fixed_rate(
                ScheduledJobSpec {
                    id: "db-auto-backup".to_string(),
                    interval: backup_interval,
                    initial_delay: Some(Duration::from_secs(300)), // 5 min after start
                    description: "Automatic database backup with health guards".to_string(),
                },
                move || {
                    let backup_svc = Arc::clone(&backup_service_for_job);
                    let registry = Arc::clone(&registry_for_backup);
                    async move {
                        info!("Running scheduled database backup");
                        match backup_svc.run_backup(crate::services::backup::BackupTrigger::Auto).await {
                            Ok(status) => {
                                info!(
                                    path = %status.path.display(),
                                    size_mb = status.size_bytes / (1024 * 1024),
                                    "Scheduled backup completed successfully"
                                );
                                registry.update_note(
                                    "db-runtime",
                                    Some(format!(
                                        "backup: {} ({} MB)",
                                        status.path.display(),
                                        status.size_bytes / (1024 * 1024)
                                    )),
                                );
                            }
                            Err(err) => {
                                warn!(error = %err, "Scheduled backup failed");
                                registry.update_note(
                                    "db-runtime",
                                    Some(format!("backup failed: {}", err)),
                                );
                            }
                        }
                        Ok::<(), anyhow::Error>(())
                    }
                },
            )?;

            info!(
                schedule = %backup_svc.config().schedule,
                "Automatic backup job registered"
            );
        }
    }

    registry.update_note(
        "scheduler",
        Some(scheduler_service_messages::STANDARD_JOBS_ACTIVE_NOTE.to_string()),
    );
    Ok(())
}

/// Parse backup schedule string to Duration
fn parse_backup_schedule(schedule: &str) -> Duration {
    match schedule.to_lowercase().as_str() {
        "hourly" => Duration::from_secs(3600),
        "daily" => Duration::from_secs(86400),
        "weekly" => Duration::from_secs(604800),
        // Could add cron parsing here
        _ => Duration::from_secs(86400), // Default to daily
    }
}

impl Drop for SchedulerService {
    fn drop(&mut self) {
        let _ = self.stop();
        self.registry.set_status(
            "scheduler",
            ServiceStatus::Stopped,
            Some(scheduler_service_messages::STOPPED_NOTE.to_string()),
        );
        info!("{}", scheduler_service_messages::SERVICE_DROPPED);
    }
}

async fn write_audit_snapshot(dir: &PathBuf, events: &[AuditEvent]) -> anyhow::Result<()> {
    fs::create_dir_all(dir)
        .await
        .with_context(|| format!("failed to prepare {}", dir.display()))?;
    let timestamp = OffsetDateTime::now_utc();
    let stamp = timestamp
        .format(&Rfc3339)
        .unwrap_or_else(|_| timestamp.unix_timestamp().to_string());
    let file_path = dir.join(format!("audit-{stamp}.json"));
    let payload = serde_json::to_vec_pretty(events)?;
    fs::write(&file_path, payload)
        .await
        .with_context(|| format!("failed to write {}", file_path.display()))?;
    Ok(())
}

fn record_token_refresh_audit(
    services: &AppServices,
    module_id: &str,
    ttl_secs: i64,
    outcome: AuditOutcome,
    error: Option<String>,
) {
    let mut metadata = AuditMetadata::default()
        .insert("job_id", "token-lease-monitor")
        .insert("module_id", module_id)
        .insert("ttl_secs", ttl_secs.to_string())
        .insert("component", "scheduler");
    if let Some(err) = error.as_deref() {
        metadata = metadata.insert("error", err);
    }
    let actor = AuditActor::System;
    match AuditEvent::builder()
        .actor(actor)
        .action("job::token-lease-refresh".to_string())
        .target(module_id.to_string())
        .outcome(outcome)
        .metadata(metadata)
        .build()
    {
        Ok(event) => {
            if let Err(err) = services.record_audit(event) {
                tracing::warn!(
                    error = %err,
                    "token lease monitor failed to append audit event"
                );
            }
        }
        Err(err) => tracing::warn!(
            error = %err,
            "token lease monitor failed to build audit event"
        ),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/services/scheduler/service_tests.rs"]
mod tests;

#[derive(Serialize, Deserialize, Default)]
struct SchedulerStateFile {
    paused: Vec<String>,
}

fn load_paused_jobs(path: &PathBuf) -> HashSet<String> {
    match std_fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<SchedulerStateFile>(&bytes)
            .map(|state| state.paused.into_iter().collect())
            .unwrap_or_default(),
        Err(_) => HashSet::new(),
    }
}
