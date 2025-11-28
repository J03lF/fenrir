use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use super::types::{ScheduledJobSnapshot, ScheduledJobSpec, SchedulerError};
use crate::infra::telemetry;
use crate::services::{DbShellService, ServiceRegistry, ServiceStatus};
use crate::utils::messages::services::scheduler::{
    debug as scheduler_debug, descriptions as scheduler_descriptions, errors as scheduler_errors,
    notes as scheduler_notes, service as scheduler_service_messages,
};

pub struct SchedulerService {
    registry: Arc<ServiceRegistry>,
    heartbeat: Mutex<Option<JoinHandle<()>>>,
    jobs: Mutex<BTreeMap<String, JobHandle>>,
}

struct JobHandle {
    stop_flag: Arc<AtomicBool>,
    notifier: Arc<Notify>,
    interval: Duration,
    description: String,
    join: JoinHandle<()>,
}

impl JobHandle {
    fn stop(self) {
        self.stop_flag.store(true, Ordering::Release);
        self.notifier.notify_waiters();
        self.join.abort();
    }
}

impl SchedulerService {
    pub fn new(registry: Arc<ServiceRegistry>) -> Self {
        Self {
            registry,
            heartbeat: Mutex::new(None),
            jobs: Mutex::new(BTreeMap::new()),
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
        let handle = tokio::spawn(async move {
            registry.set_status(
                "scheduler",
                ServiceStatus::Active,
                Some(scheduler_service_messages::HEARTBEAT_ACTIVE_NOTE.to_string()),
            );
            info!("{}", scheduler_service_messages::HEARTBEAT_LOOP_STARTED);

            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                registry.update_note(
                    "scheduler",
                    Some(scheduler_service_messages::HEARTBEAT_OK_NOTE.to_string()),
                );
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
        while let Some((id, handle)) = guard.pop_first() {
            handle.stop();
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
        F: FnMut() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    {
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

        let ScheduledJobSpec {
            id,
            interval,
            initial_delay,
            description,
        } = spec;

        let mut guard = self.jobs.lock().expect("scheduler jobs lock");
        if guard.contains_key(&id) {
            return Err(SchedulerError::JobAlreadyExists(id.clone()));
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        let notifier = Arc::new(Notify::new());
        let job_id = id.clone();
        let registry = Arc::clone(&self.registry);
        let stop_clone = Arc::clone(&stop_flag);
        let notifier_clone = Arc::clone(&notifier);

        let join = tokio::spawn(async move {
            let mut job_fn = job;
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
                if let Err(err) = job_fn().await {
                    error!(
                        job = %job_id,
                        error = %err,
                        "{}",
                        scheduler_errors::JOB_FAILED
                    );
                    registry.update_note(
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

        guard.insert(
            id.clone(),
            JobHandle {
                stop_flag,
                notifier,
                interval,
                description,
                join,
            },
        );
        self.registry.update_note(
            "scheduler",
            Some(scheduler_notes::job_active(&id, interval.as_secs())),
        );
        Ok(())
    }

    pub fn cancel_job(&self, id: &str) -> bool {
        let mut guard = self.jobs.lock().expect("scheduler jobs lock");
        if let Some(handle) = guard.remove(id) {
            handle.stop();
            self.registry
                .update_note("scheduler", Some(scheduler_notes::job_stopped(id)));
            true
        } else {
            false
        }
    }

    pub fn jobs(&self) -> Vec<ScheduledJobSnapshot> {
        let active = self.is_running();
        self.jobs
            .lock()
            .expect("scheduler jobs lock")
            .iter()
            .map(|(id, handle)| ScheduledJobSnapshot {
                id: id.clone(),
                interval: handle.interval,
                description: handle.description.clone(),
                active,
            })
            .collect()
    }
}

pub fn install_default_jobs(
    scheduler: &SchedulerService,
    registry: Arc<ServiceRegistry>,
    db_shell: Arc<DbShellService>,
) -> Result<(), SchedulerError> {
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
            async move {
                let session = db_shell.create_session();
                match session.ping().await {
                    Ok(_) => {
                        registry.set_status(
                            "db-shell",
                            ServiceStatus::Active,
                            Some(scheduler_service_messages::DB_AVAILABLE_NOTE.to_string()),
                        );
                    }
                    Err(err) => {
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

    registry.update_note(
        "scheduler",
        Some(scheduler_service_messages::STANDARD_JOBS_ACTIVE_NOTE.to_string()),
    );
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{ServiceDescriptor, ServiceKind, ServiceRegistry, ServiceStatus};

    #[tokio::test]
    async fn schedules_and_cancels_jobs() {
        let registry = Arc::new(ServiceRegistry::new());
        registry.register(
            ServiceDescriptor::new(
                "scheduler",
                "Scheduler",
                "test scheduler",
                ServiceKind::BackgroundJob,
            ),
            ServiceStatus::Starting,
            None,
        );
        let scheduler = SchedulerService::new(Arc::clone(&registry));
        assert!(scheduler.start());

        let spec = ScheduledJobSpec {
            id: "heartbeat".to_string(),
            interval: Duration::from_millis(10),
            initial_delay: None,
            description: "heartbeat".to_string(),
        };

        scheduler
            .schedule_fixed_rate(spec, || async { Ok(()) })
            .expect("schedule job");

        tokio::time::sleep(Duration::from_millis(25)).await;

        assert_eq!(scheduler.jobs().len(), 1);
        assert!(scheduler.cancel_job("heartbeat"));
        assert!(scheduler.stop());
    }
}
