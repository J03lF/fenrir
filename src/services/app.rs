use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::audit::{AuditError, AuditEvent, AuditLog};
use crate::infra::logging::ReloadHandle;
use crate::security::identity::IdentityProvider;
use crate::security::manager::{AuditSink, SecurityManager};
use crate::utils::messages::services::app::{attach, logs as app_logs};
use futures::future::BoxFuture;
use once_cell::sync::OnceCell;
use tokio::sync::broadcast;

use super::backup::BackupService;
use super::db_shell::DbShellService;
use super::diagnostics::{ServiceDiagnostics, ServiceMetricSnapshot};
use super::jobs::{JobLogError, JobLogSnapshot};
use super::managed::{
    block_on_managed, ClosureManagedService, ManagedService, ServiceControlError,
    ServiceControlOutcome,
};
use super::module::ModuleService;
use super::registry::ServiceRegistry;
use super::scheduler::{JobControlOutcome, ScheduledJobSnapshot, SchedulerError, SchedulerService};
use super::security::{InstrumentedIdentityProvider, SessionService};
use super::token_exchange::TokenExchangeService;
use super::types::{ServiceActionReport, ServiceTag};

pub struct AppServices {
    pub db_shell: Arc<DbShellService>,
    pub scheduler: Arc<SchedulerService>,
    diagnostics: Arc<ServiceDiagnostics>,
    registry: Arc<ServiceRegistry>,
    managed: RwLock<BTreeMap<&'static str, Arc<dyn ManagedService>>>,
    logging: RwLock<Option<ReloadHandle>>,
    audit_log: Arc<dyn AuditLog>,
    audit_bus: broadcast::Sender<AuditEvent>,
    module_service: OnceCell<Arc<ModuleService>>,
    token_exchange: OnceCell<Arc<TokenExchangeService>>,
    security: OnceCell<Arc<SecurityManager>>,
    session: OnceCell<Arc<SessionService>>,
    identity: OnceCell<Arc<dyn IdentityProvider>>,
    db_runtime: OnceCell<Arc<crate::infra::db::runtime::DbRuntimeSupervisor>>,
    backup_service: OnceCell<Arc<BackupService>>,
}

impl AppServices {
    pub fn new(
        db_shell: Arc<DbShellService>,
        scheduler: Arc<SchedulerService>,
        registry: Arc<ServiceRegistry>,
        audit_log: Arc<dyn AuditLog>,
        diagnostics: Arc<ServiceDiagnostics>,
    ) -> Self {
        let (audit_bus, _) = broadcast::channel(256);
        Self {
            db_shell,
            scheduler,
            diagnostics,
            registry,
            managed: RwLock::new(BTreeMap::new()),
            logging: RwLock::new(None),
            audit_log,
            audit_bus,
            module_service: OnceCell::new(),
            token_exchange: OnceCell::new(),
            security: OnceCell::new(),
            session: OnceCell::new(),
            identity: OnceCell::new(),
            db_runtime: OnceCell::new(),
            backup_service: OnceCell::new(),
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

    pub fn attach_token_exchange(
        &self,
        service: Arc<TokenExchangeService>,
    ) -> Result<(), &'static str> {
        self.token_exchange
            .set(service)
            .map_err(|_| attach::TOKEN_EXCHANGE_ALREADY_ATTACHED)
    }

    pub fn token_exchange_service(&self) -> Option<Arc<TokenExchangeService>> {
        self.token_exchange.get().cloned()
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
        let instrumented: Arc<dyn IdentityProvider> = Arc::new(InstrumentedIdentityProvider::new(
            identity,
            self.diagnostics(),
        ));
        self.identity
            .set(instrumented)
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

    pub fn attach_db_runtime(
        &self,
        runtime: Arc<crate::infra::db::runtime::DbRuntimeSupervisor>,
    ) -> Result<(), &'static str> {
        self.db_runtime
            .set(runtime)
            .map_err(|_| attach::DB_RUNTIME_ALREADY_ATTACHED)
    }

    pub fn db_runtime(&self) -> Option<Arc<crate::infra::db::runtime::DbRuntimeSupervisor>> {
        self.db_runtime.get().cloned()
    }

    pub fn attach_backup_service(&self, service: Arc<BackupService>) -> Result<(), &'static str> {
        self.backup_service
            .set(service)
            .map_err(|_| attach::BACKUP_SERVICE_ALREADY_ATTACHED)
    }

    pub fn backup_service(&self) -> Option<Arc<BackupService>> {
        self.backup_service.get().cloned()
    }

    pub fn session_service(&self) -> Option<Arc<SessionService>> {
        self.session.get().cloned()
    }

    pub fn registry(&self) -> Arc<ServiceRegistry> {
        Arc::clone(&self.registry)
    }

    pub fn db_runtime_status(&self) -> Option<crate::infra::db::runtime::RuntimeStatus> {
        self.db_runtime.get().and_then(|rt| rt.status_snapshot())
    }

    pub fn db_runtime_logs(&self, tail: usize) -> Vec<String> {
        self.db_runtime
            .get()
            .map(|rt| rt.logs(tail))
            .unwrap_or_default()
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
        // Filter out noisy system authorize events that flood the audit log

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

    pub fn diagnostics(&self) -> Arc<ServiceDiagnostics> {
        Arc::clone(&self.diagnostics)
    }

    pub fn service_diagnostics(&self, id: &str) -> Option<ServiceMetricSnapshot> {
        self.diagnostics.snapshot(id)
    }

    pub fn service_diagnostics_snapshot(&self) -> HashMap<String, ServiceMetricSnapshot> {
        self.diagnostics.snapshot_all()
    }

    pub fn scheduler_jobs(&self) -> Vec<ScheduledJobSnapshot> {
        self.diagnostics.record_heartbeat("jobs-control");
        self.scheduler.jobs()
    }

    pub fn scheduler_job(&self, id: &str) -> Option<ScheduledJobSnapshot> {
        self.diagnostics.record_heartbeat("jobs-control");
        self.scheduler.job(id)
    }

    pub fn restart_job(&self, id: &str) -> Result<JobControlOutcome, SchedulerError> {
        let started_at = Instant::now();
        let result = self.scheduler.restart_job(id);
        self.record_jobs_control_probe(started_at, result.is_ok());
        result
    }

    pub fn pause_job(&self, id: &str) -> Result<JobControlOutcome, SchedulerError> {
        let started_at = Instant::now();
        let result = self.scheduler.pause_job(id);
        self.record_jobs_control_probe(started_at, result.is_ok());
        result
    }

    pub fn resume_job(&self, id: &str) -> Result<JobControlOutcome, SchedulerError> {
        let started_at = Instant::now();
        let result = self.scheduler.resume_job(id);
        self.record_jobs_control_probe(started_at, result.is_ok());
        result
    }

    pub fn job_logs(&self, job_id: &str, tail: usize) -> Result<JobLogSnapshot, JobLogError> {
        let started_at = Instant::now();
        let result = crate::services::jobs::collect_job_logs(job_id, tail);
        self.record_jobs_control_probe(started_at, result.is_ok());
        result
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

impl AppServices {
    fn record_jobs_control_probe(&self, started_at: Instant, success: bool) {
        let latency_ms = started_at.elapsed().as_secs_f64() * 1000.0;
        self.diagnostics
            .record_probe("jobs-control", latency_ms, success);
    }
}
