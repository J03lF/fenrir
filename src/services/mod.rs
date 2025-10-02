pub mod db_shell;
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
use tokio::task;

use crate::infra::logging::ReloadHandle;

pub use db_shell::DbShellService;
pub use scheduler::SchedulerService;
pub use ticket::TicketService;
pub use user::UserService;

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

    pub fn get(&self, id: &str) -> Option<ServiceSnapshot> {
        match self.inner.read() {
            Ok(guard) => guard.get(id).map(|record| ServiceSnapshot {
                descriptor: record.descriptor,
                status: record.status,
                since: record.since,
                note: record.note.clone(),
            }),
            Err(_) => None,
        }
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
}

impl AppServices {
    pub fn new(
        db_shell: Arc<DbShellService>,
        scheduler: Arc<SchedulerService>,
        ticket: Arc<TicketService>,
        user: Arc<UserService>,
        registry: Arc<ServiceRegistry>,
    ) -> Self {
        Self {
            db_shell,
            scheduler,
            ticket,
            user,
            registry,
            managed: RwLock::new(BTreeMap::new()),
            logging: RwLock::new(None),
        }
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
