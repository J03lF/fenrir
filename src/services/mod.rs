pub mod db_shell;
pub mod module;
pub mod scheduler;
pub mod ticket;
pub mod user;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use anyhow::Error as AnyError;
use async_trait::async_trait;
use futures::future::BoxFuture;
use thiserror::Error;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::broadcast;
use tokio::task;

use crate::audit::{AuditError, AuditEvent, AuditLog};
use crate::infra::logging::ReloadHandle;
use once_cell::sync::OnceCell;

pub use db_shell::DbShellService;
pub use module::ModuleService;
pub use scheduler::SchedulerService;
pub use ticket::TicketService;
pub use user::UserService;

#[derive(Debug)]
pub struct ServiceActionReport {
    pub id: String,
    pub result: Result<ServiceControlOutcome, ServiceControlError>,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServiceTag {
    Core,
    Platform,
    Auxiliary,
}

impl ServiceTag {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceTag::Core => "core",
            ServiceTag::Platform => "platform",
            ServiceTag::Auxiliary => "auxiliary",
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
    pub critical: bool,
    pub tags: &'static [ServiceTag],
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
            critical: false,
            tags: &[],
        }
    }

    pub const fn critical(self) -> Self {
        Self {
            critical: true,
            ..self
        }
    }

    pub const fn with_tags(self, tags: &'static [ServiceTag]) -> Self {
        Self { tags, ..self }
    }

    pub fn has_tag(&self, tag: ServiceTag) -> bool {
        self.tags.iter().any(|t| t == &tag)
    }
}

#[derive(Clone, Debug)]
pub struct ServiceSnapshot {
    pub descriptor: ServiceDescriptor,
    pub status: ServiceStatus,
    pub since: SystemTime,
    pub note: Option<String>,
}

#[async_trait]
pub trait ManagedService: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    async fn start(self: Arc<Self>) -> anyhow::Result<bool>;
    async fn stop(self: Arc<Self>, force: bool) -> anyhow::Result<bool>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceControlOutcome {
    Started,
    AlreadyRunning,
    Stopped,
    AlreadyStopped,
    Restarted,
}

impl ServiceControlOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceControlOutcome::Started => "started",
            ServiceControlOutcome::AlreadyRunning => "already_running",
            ServiceControlOutcome::Stopped => "stopped",
            ServiceControlOutcome::AlreadyStopped => "already_stopped",
            ServiceControlOutcome::Restarted => "restarted",
        }
    }
}

#[derive(Error, Debug)]
pub enum ServiceControlError {
    #[error("service `{0}` ist unbekannt")]
    UnknownService(String),
    #[error("service `{0}` unterstützt keine Laufzeitsteuerung")]
    NotControllable(String),
    #[error("service `{0}` ist als kritisch markiert – --force erforderlich")]
    ForceRequired(String),
    #[error("service `{0}` ist als core markiert und kann nicht gestoppt werden")]
    CoreLocked(String),
    #[error("operation für service `{id}` fehlgeschlagen: {source}")]
    OperationFailed {
        id: String,
        #[source]
        source: AnyError,
    },
}

struct ServiceRecord {
    descriptor: ServiceDescriptor,
    status: ServiceStatus,
    since: SystemTime,
    note: Option<String>,
}

#[derive(Clone)]
pub struct ServiceRegistry {
    inner: Arc<RwLock<BTreeMap<&'static str, ServiceRecord>>>,
    events: broadcast::Sender<ServiceSnapshot>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceRegistry {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(RwLock::new(BTreeMap::new())),
            events,
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
        let snapshot = if let Ok(mut guard) = self.inner.write() {
            guard.insert(descriptor.id, record);
            guard.get(descriptor.id).map(snapshot_from_record)
        } else {
            tracing::error!(service_id = descriptor.id, "service registry lock poisoned");
            None
        };
        if let Some(snapshot) = snapshot {
            let _ = self.events.send(snapshot);
        }
    }

    pub fn set_status(&self, id: &str, status: ServiceStatus, note: impl Into<Option<String>>) {
        let snapshot = match self.inner.write() {
            Ok(mut guard) => {
                if let Some(record) = guard.get_mut(id) {
                    record.status = status;
                    record.note = note.into();
                    record.since = SystemTime::now();
                    Some(snapshot_from_record(record))
                } else {
                    tracing::warn!(
                        service_id = id,
                        "versuch, unbekannten Service zu aktualisieren"
                    );
                    None
                }
            }
            Err(_) => {
                tracing::error!(service_id = id, "service registry lock poisoned");
                None
            }
        };
        if let Some(snapshot) = snapshot {
            let _ = self.events.send(snapshot);
        }
    }

    pub fn update_note(&self, id: &str, note: impl Into<Option<String>>) {
        let snapshot = if let Ok(mut guard) = self.inner.write() {
            if let Some(record) = guard.get_mut(id) {
                record.note = note.into();
                Some(snapshot_from_record(record))
            } else {
                None
            }
        } else {
            None
        };
        if let Some(snapshot) = snapshot {
            let _ = self.events.send(snapshot);
        }
    }

    pub fn snapshot(&self) -> Vec<ServiceSnapshot> {
        match self.inner.read() {
            Ok(guard) => guard.values().map(snapshot_from_record).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<ServiceSnapshot> {
        match self.inner.read() {
            Ok(guard) => guard.get(id).map(snapshot_from_record),
            Err(_) => None,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServiceSnapshot> {
        self.events.subscribe()
    }
}

fn snapshot_from_record(record: &ServiceRecord) -> ServiceSnapshot {
    ServiceSnapshot {
        descriptor: record.descriptor,
        status: record.status,
        since: record.since,
        note: record.note.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcasts_on_register_and_update() {
        let registry = ServiceRegistry::new();
        let mut receiver = registry.subscribe();
        let descriptor =
            ServiceDescriptor::new("test-service", "Test Service", "Test", ServiceKind::Other);
        registry.register(descriptor, ServiceStatus::Starting, None::<String>);
        let event = receiver.try_recv().expect("expected register event");
        assert_eq!(event.descriptor.id, "test-service");
        assert_eq!(event.status, ServiceStatus::Starting);

        registry.set_status(
            "test-service",
            ServiceStatus::Active,
            Some("ok".to_string()),
        );
        let event = receiver.try_recv().expect("expected status update");
        assert_eq!(event.status, ServiceStatus::Active);
        assert_eq!(event.note.as_deref(), Some("ok"));
    }
}

pub struct AppServices {
    pub db_shell: Arc<DbShellService>,
    pub scheduler: Arc<SchedulerService>,
    pub ticket: Arc<TicketService>,
    pub user: Arc<UserService>,
    registry: Arc<ServiceRegistry>,
    managed: RwLock<BTreeMap<&'static str, Arc<dyn ManagedService>>>,
    logging: RwLock<Option<ReloadHandle>>,
    audit_log: Arc<dyn AuditLog>,
    audit_bus: broadcast::Sender<AuditEvent>,
    module_service: OnceCell<Arc<ModuleService>>,
}

impl AppServices {
    pub fn new(
        db_shell: Arc<DbShellService>,
        scheduler: Arc<SchedulerService>,
        ticket: Arc<TicketService>,
        user: Arc<UserService>,
        registry: Arc<ServiceRegistry>,
        audit_log: Arc<dyn AuditLog>,
    ) -> Self {
        let (audit_bus, _) = broadcast::channel(256);
        Self {
            db_shell,
            scheduler,
            ticket,
            user,
            registry,
            managed: RwLock::new(BTreeMap::new()),
            logging: RwLock::new(None),
            audit_log,
            audit_bus,
            module_service: OnceCell::new(),
        }
    }

    pub fn attach_module_service(&self, service: Arc<ModuleService>) -> Result<(), &'static str> {
        self.module_service
            .set(service)
            .map_err(|_| "module service already attached")
    }

    pub fn module_service(&self) -> Option<Arc<ModuleService>> {
        self.module_service.get().cloned()
    }

    pub fn registry(&self) -> Arc<ServiceRegistry> {
        Arc::clone(&self.registry)
    }

    pub fn set_logging_handle(&self, handle: ReloadHandle) {
        if let Ok(mut guard) = self.logging.write() {
            *guard = Some(handle);
        } else {
            tracing::error!("logging handle lock poisoned");
        }
    }

    pub fn logging_handle(&self) -> Option<ReloadHandle> {
        self.logging.read().ok().and_then(|guard| guard.clone())
    }

    pub fn audit_store(&self) -> Arc<dyn AuditLog> {
        Arc::clone(&self.audit_log)
    }

    pub fn record_audit(&self, event: AuditEvent) -> Result<(), AuditError> {
        self.audit_log.append(event.clone())?;
        let _ = self.audit_bus.send(event);
        Ok(())
    }

    pub fn audit_recent(&self, limit: usize) -> Result<Vec<AuditEvent>, AuditError> {
        self.audit_log.recent(limit)
    }

    pub fn audit_subscribe(&self) -> broadcast::Receiver<AuditEvent> {
        self.audit_bus.subscribe()
    }

    pub fn scheduler_service(&self) -> Arc<SchedulerService> {
        Arc::clone(&self.scheduler)
    }

    pub fn register_runtime_service<T>(&self, service: Arc<T>)
    where
        T: ManagedService,
    {
        let service: Arc<dyn ManagedService> = service;
        if let Ok(mut guard) = self.managed.write() {
            guard.insert(service.id(), service);
        } else {
            tracing::error!("managed service registry lock poisoned");
        }
    }

    pub fn register_dynamic_service<FStart, FStop>(
        &self,
        id: &'static str,
        start: FStart,
        stop: FStop,
    ) where
        FStart: Fn() -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync + 'static,
        FStop: Fn(bool) -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync + 'static,
    {
        let service = ClosureManagedService::new(id, start, stop);
        self.register_runtime_service(service);
    }

    fn controllable_non_core_ids(&self) -> Vec<&'static str> {
        let managed = self
            .managed
            .read()
            .map(|guard| guard.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        self.registry
            .snapshot()
            .into_iter()
            .filter(|snapshot| {
                managed.contains(&snapshot.descriptor.id)
                    && !snapshot.descriptor.has_tag(ServiceTag::Core)
            })
            .map(|snapshot| snapshot.descriptor.id)
            .collect()
    }

    fn managed_service(&self, id: &str) -> Option<Arc<dyn ManagedService>> {
        self.managed
            .read()
            .ok()
            .and_then(|guard| guard.get(id).cloned())
    }

    pub fn start_service(&self, id: &str) -> Result<ServiceControlOutcome, ServiceControlError> {
        let descriptor = self
            .registry
            .get(id)
            .ok_or_else(|| ServiceControlError::UnknownService(id.to_string()))?;
        let handle = self
            .managed_service(id)
            .ok_or_else(|| ServiceControlError::NotControllable(id.to_string()))?;
        let fut = Arc::clone(&handle).start();
        match block_on_managed(fut) {
            Ok(true) => Ok(ServiceControlOutcome::Started),
            Ok(false) => Ok(ServiceControlOutcome::AlreadyRunning),
            Err(err) => Err(ServiceControlError::OperationFailed {
                id: descriptor.descriptor.id.to_string(),
                source: err,
            }),
        }
    }

    pub fn stop_service(
        &self,
        id: &str,
        force: bool,
    ) -> Result<ServiceControlOutcome, ServiceControlError> {
        let snapshot = self
            .registry
            .get(id)
            .ok_or_else(|| ServiceControlError::UnknownService(id.to_string()))?;
        if snapshot.descriptor.critical && !force {
            return Err(ServiceControlError::ForceRequired(id.to_string()));
        }
        if snapshot.descriptor.has_tag(ServiceTag::Core) {
            return Err(ServiceControlError::CoreLocked(id.to_string()));
        }
        let handle = self
            .managed_service(id)
            .ok_or_else(|| ServiceControlError::NotControllable(id.to_string()))?;
        let fut = Arc::clone(&handle).stop(force);
        match block_on_managed(fut) {
            Ok(true) => Ok(ServiceControlOutcome::Stopped),
            Ok(false) => Ok(ServiceControlOutcome::AlreadyStopped),
            Err(err) => Err(ServiceControlError::OperationFailed {
                id: id.to_string(),
                source: err,
            }),
        }
    }

    pub fn restart_service(
        &self,
        id: &str,
        force: bool,
    ) -> Result<ServiceControlOutcome, ServiceControlError> {
        let snapshot = self
            .registry
            .get(id)
            .ok_or_else(|| ServiceControlError::UnknownService(id.to_string()))?;
        if snapshot.descriptor.critical && !force {
            return Err(ServiceControlError::ForceRequired(id.to_string()));
        }
        if snapshot.descriptor.has_tag(ServiceTag::Core) {
            return Err(ServiceControlError::CoreLocked(id.to_string()));
        }
        let _ = self.stop_service(id, force)?;
        match self.start_service(id)? {
            ServiceControlOutcome::Started | ServiceControlOutcome::AlreadyRunning => {
                Ok(ServiceControlOutcome::Restarted)
            }
            other => Ok(other),
        }
    }

    pub fn stop_all_non_core(&self, force: bool) -> Vec<ServiceActionReport> {
        self.controllable_non_core_ids()
            .into_iter()
            .map(|id| ServiceActionReport {
                id: id.to_string(),
                result: self.stop_service(id, force),
            })
            .collect()
    }

    pub fn start_all_non_core(&self) -> Vec<ServiceActionReport> {
        self.controllable_non_core_ids()
            .into_iter()
            .map(|id| ServiceActionReport {
                id: id.to_string(),
                result: self.start_service(id),
            })
            .collect()
    }

    pub fn restart_all_non_core(&self, force: bool) -> Vec<ServiceActionReport> {
        self.controllable_non_core_ids()
            .into_iter()
            .map(|id| ServiceActionReport {
                id: id.to_string(),
                result: self.restart_service(id, force),
            })
            .collect()
    }
}

struct ClosureManagedService {
    id: &'static str,
    start: Arc<dyn Fn() -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync>,
    stop: Arc<dyn Fn(bool) -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync>,
}

impl ClosureManagedService {
    fn new<FStart, FStop>(id: &'static str, start: FStart, stop: FStop) -> Arc<Self>
    where
        FStart: Fn() -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync + 'static,
        FStop: Fn(bool) -> BoxFuture<'static, anyhow::Result<bool>> + Send + Sync + 'static,
    {
        Arc::new(Self {
            id,
            start: Arc::new(start),
            stop: Arc::new(stop),
        })
    }
}

#[async_trait]
impl ManagedService for ClosureManagedService {
    fn id(&self) -> &'static str {
        self.id
    }

    async fn start(self: Arc<Self>) -> anyhow::Result<bool> {
        (self.start)().await
    }

    async fn stop(self: Arc<Self>, force: bool) -> anyhow::Result<bool> {
        (self.stop)(force).await
    }
}

fn block_on_managed<F, T>(future: F) -> T
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    match Handle::try_current() {
        Ok(handle) => task::block_in_place(|| handle.block_on(future)),
        Err(_) => {
            let runtime = Runtime::new().expect("tokio runtime for managed service");
            runtime.block_on(future)
        }
    }
}
