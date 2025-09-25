pub mod db_shell;
pub mod scheduler;
pub mod ticket;
pub mod user;

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

pub use db_shell::DbShellService;
pub use scheduler::SchedulerService;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    Infrastructure,
    Transport,
    BackgroundJob,
    Cli,
    Security,
    Storage,
    Other,
}

impl ServiceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceKind::Infrastructure => "infra",
            ServiceKind::Transport => "transport",
            ServiceKind::BackgroundJob => "job",
            ServiceKind::Cli => "cli",
            ServiceKind::Security => "security",
            ServiceKind::Storage => "storage",
            ServiceKind::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceStatus {
    Starting,
    Active,
    Degraded,
    Failed,
    Standby,
    Stopped,
}

impl ServiceStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ServiceStatus::Starting => "starting",
            ServiceStatus::Active => "active",
            ServiceStatus::Degraded => "degraded",
            ServiceStatus::Failed => "failed",
            ServiceStatus::Standby => "standby",
            ServiceStatus::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ServiceDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub kind: ServiceKind,
}

impl ServiceDescriptor {
    pub const fn new(
        id: &'static str,
        name: &'static str,
        description: &'static str,
        kind: ServiceKind,
    ) -> Self {
        Self {
            id,
            name,
            description,
            kind,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceSnapshot {
    pub descriptor: ServiceDescriptor,
    pub status: ServiceStatus,
    pub since: SystemTime,
    pub note: Option<String>,
}

struct ServiceRecord {
    descriptor: ServiceDescriptor,
    status: ServiceStatus,
    since: SystemTime,
    note: Option<String>,
}

#[derive(Clone, Default)]
pub struct ServiceRegistry {
    inner: Arc<RwLock<BTreeMap<&'static str, ServiceRecord>>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub fn register(
        &self,
        descriptor: ServiceDescriptor,
        status: ServiceStatus,
        note: impl Into<Option<String>>,
    ) {
        let record = ServiceRecord {
            descriptor,
            status,
            since: SystemTime::now(),
            note: note.into(),
        };
        if let Ok(mut guard) = self.inner.write() {
            guard.insert(descriptor.id, record);
        } else {
            tracing::error!(service_id = descriptor.id, "service registry lock poisoned");
        }
    }

    pub fn set_status(&self, id: &str, status: ServiceStatus, note: impl Into<Option<String>>) {
        match self.inner.write() {
            Ok(mut guard) => {
                if let Some(record) = guard.get_mut(id) {
                    record.status = status;
                    record.note = note.into();
                    record.since = SystemTime::now();
                } else {
                    tracing::warn!(
                        service_id = id,
                        "versuch, unbekannten Service zu aktualisieren"
                    );
                }
            }
            Err(_) => tracing::error!(service_id = id, "service registry lock poisoned"),
        }
    }

    pub fn update_note(&self, id: &str, note: impl Into<Option<String>>) {
        if let Ok(mut guard) = self.inner.write() {
            if let Some(record) = guard.get_mut(id) {
                record.note = note.into();
            }
        }
    }

    pub fn snapshot(&self) -> Vec<ServiceSnapshot> {
        match self.inner.read() {
            Ok(guard) => guard
                .values()
                .map(|record| ServiceSnapshot {
                    descriptor: record.descriptor,
                    status: record.status,
                    since: record.since,
                    note: record.note.clone(),
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct AppServices {
    pub db_shell: Arc<DbShellService>,
    pub scheduler: Arc<SchedulerService>,
    registry: Arc<ServiceRegistry>,
}

impl AppServices {
    pub fn new(
        db_shell: Arc<DbShellService>,
        scheduler: Arc<SchedulerService>,
        registry: Arc<ServiceRegistry>,
    ) -> Self {
        Self {
            db_shell,
            scheduler,
            registry,
        }
    }

    pub fn registry(&self) -> Arc<ServiceRegistry> {
        Arc::clone(&self.registry)
    }
}
