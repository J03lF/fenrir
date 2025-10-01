use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::info;

use crate::services::{ServiceRegistry, ServiceStatus};

pub struct SchedulerService {
    registry: Arc<ServiceRegistry>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl SchedulerService {
    pub fn new(registry: Arc<ServiceRegistry>) -> Self {
        Self {
            registry,
            handle: Mutex::new(None),
        }
    }

    pub fn start(&self) {
        {
            let guard = self.handle.lock().expect("scheduler handle lock");
            if guard.is_some() {
                info!("scheduler already running, skipping start request");
                return;
            }
        }
        self.registry.set_status(
            "scheduler",
            ServiceStatus::Starting,
            Some("initialisiere".to_string()),
        );
        let registry = Arc::clone(&self.registry);
        let handle = tokio::spawn(async move {
            registry.set_status(
                "scheduler",
                ServiceStatus::Active,
                Some("Heartbeat aktiv".to_string()),
            );
            info!("scheduler event loop started");
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                registry.update_note("scheduler", Some("Heartbeat OK".to_string()));
                tracing::debug!(target: "scheduler", "heartbeat sent");
            }
        });
        let mut guard = self.handle.lock().expect("scheduler handle lock");
        *guard = Some(handle);
    }
}

impl Drop for SchedulerService {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.handle.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
                self.registry.set_status(
                    "scheduler",
                    ServiceStatus::Stopped,
                    Some("gestoppt".to_string()),
                );
                info!("scheduler service dropped and task aborted");
            }
        }
    }
}
