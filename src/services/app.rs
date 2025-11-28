use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use crate::audit::{AuditError, AuditEvent, AuditLog};
use crate::infra::logging::ReloadHandle;
use crate::security::identity::IdentityProvider;
use crate::security::manager::{AuditSink, SecurityManager};
use crate::utils::messages::services::app::{attach, logs as app_logs};
use futures::future::BoxFuture;
use once_cell::sync::OnceCell;
use tokio::sync::broadcast;

use super::db_shell::DbShellService;
use super::managed::{
    block_on_managed, ClosureManagedService, ManagedService, ServiceControlError,
    ServiceControlOutcome,
};
use super::module::ModuleService;
use super::registry::ServiceRegistry;
use super::scheduler::SchedulerService;
use super::security::SessionService;
use super::types::{ServiceActionReport, ServiceTag};

pub struct AppServices {
    pub db_shell: Arc<DbShellService>,
    pub scheduler: Arc<SchedulerService>,
    registry: Arc<ServiceRegistry>,
    managed: RwLock<BTreeMap<&'static str, Arc<dyn ManagedService>>>,
    logging: RwLock<Option<ReloadHandle>>,
    audit_log: Arc<dyn AuditLog>,
    audit_bus: broadcast::Sender<AuditEvent>,
    module_service: OnceCell<Arc<ModuleService>>,
    security: OnceCell<Arc<SecurityManager>>,
    session: OnceCell<Arc<SessionService>>,
    identity: OnceCell<Arc<dyn IdentityProvider>>,
}

impl AppServices {
    pub fn new(
        db_shell: Arc<DbShellService>,
        scheduler: Arc<SchedulerService>,
        registry: Arc<ServiceRegistry>,
        audit_log: Arc<dyn AuditLog>,
    ) -> Self {
        let (audit_bus, _) = broadcast::channel(256);
        Self {
            db_shell,
            scheduler,
            registry,
            managed: RwLock::new(BTreeMap::new()),
            logging: RwLock::new(None),
            audit_log,
            audit_bus,
            module_service: OnceCell::new(),
            security: OnceCell::new(),
            session: OnceCell::new(),
            identity: OnceCell::new(),
        }
    }

    pub fn attach_module_service(&self, service: Arc<ModuleService>) -> Result<(), &'static str> {
        self.module_service
            .set(service)
            .map_err(|_| attach::MODULE_SERVICE_ALREADY_ATTACHED)
    }

    pub fn module_service(&self) -> Option<Arc<ModuleService>> {
        self.module_service.get().cloned()
    }

    pub fn attach_security(&self, manager: Arc<SecurityManager>) -> Result<(), &'static str> {
        self.security
            .set(manager)
            .map_err(|_| attach::SECURITY_MANAGER_ALREADY_ATTACHED)
    }

    pub fn security_manager(&self) -> Option<Arc<SecurityManager>> {
        self.security.get().cloned()
    }

    pub fn attach_identity(&self, identity: Arc<dyn IdentityProvider>) -> Result<(), &'static str> {
        self.identity
            .set(identity)
            .map_err(|_| attach::IDENTITY_SERVICE_ALREADY_ATTACHED)
    }

    pub fn identity(&self) -> Option<Arc<dyn IdentityProvider>> {
        self.identity.get().cloned()
    }

    pub fn attach_session(&self, service: Arc<SessionService>) -> Result<(), &'static str> {
        self.session
            .set(service)
            .map_err(|_| attach::SESSION_SERVICE_ALREADY_ATTACHED)
    }

    pub fn session_service(&self) -> Option<Arc<SessionService>> {
        self.session.get().cloned()
    }

    pub fn registry(&self) -> Arc<ServiceRegistry> {
        Arc::clone(&self.registry)
    }

    pub fn set_logging_handle(&self, handle: ReloadHandle) {
        if let Ok(mut guard) = self.logging.write() {
            *guard = Some(handle);
        } else {
            tracing::error!("{}", app_logs::LOGGING_HANDLE_LOCK_POISONED);
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
            tracing::error!("{}", app_logs::MANAGED_REGISTRY_LOCK_POISONED);
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

    fn controllable_non_core_ids(&self) -> Vec<String> {
        let managed = self
            .managed
            .read()
            .map(|guard| guard.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        self.registry
            .snapshot()
            .into_iter()
            .filter(|snapshot| {
                managed.contains(&snapshot.descriptor.id.as_str())
                    && !snapshot.descriptor.has_tag(ServiceTag::Core)
            })
            .map(|snapshot| snapshot.descriptor.id.clone())
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
                id: id.clone(),
                result: self.stop_service(&id, force),
            })
            .collect()
    }

    pub fn start_all_non_core(&self) -> Vec<ServiceActionReport> {
        self.controllable_non_core_ids()
            .into_iter()
            .map(|id| ServiceActionReport {
                id: id.clone(),
                result: self.start_service(&id),
            })
            .collect()
    }

    pub fn restart_all_non_core(&self, force: bool) -> Vec<ServiceActionReport> {
        self.controllable_non_core_ids()
            .into_iter()
            .map(|id| ServiceActionReport {
                id: id.clone(),
                result: self.restart_service(&id, force),
            })
            .collect()
    }
}

impl AuditSink for AppServices {
    fn record(&self, event: AuditEvent) -> Result<(), AuditError> {
        self.record_audit(event)
    }
}
