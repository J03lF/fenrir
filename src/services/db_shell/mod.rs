use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::domain::db::{
    DbAdminPort, DbEngine, DbError, DbExecutionResult, DbResult, DbTable, DbTableSchema,
};
use crate::services::{ManagedService, ServiceRegistry, ServiceStatus};
use async_trait::async_trait;

pub struct DbShellService {
    default_engine: DbEngine,
    adapters: Arc<BTreeMap<DbEngine, Arc<dyn DbAdminPort>>>,
    enabled: AtomicBool,
}

impl Clone for DbShellService {
    fn clone(&self) -> Self {
        Self {
            default_engine: self.default_engine,
            adapters: Arc::clone(&self.adapters),
            enabled: AtomicBool::new(self.is_enabled()),
        }
    }
}

impl DbShellService {
    pub fn new(
        default_engine: DbEngine,
        adapters: BTreeMap<DbEngine, Arc<dyn DbAdminPort>>,
    ) -> Result<Self, DbError> {
        if !adapters.contains_key(&default_engine) {
            return Err(DbError::EngineNotConfigured {
                engine: default_engine,
            });
        }
        Ok(Self {
            default_engine,
            adapters: Arc::new(adapters),
            enabled: AtomicBool::new(true),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    pub fn available_engines(&self) -> Vec<DbEngine> {
        self.adapters.keys().copied().collect()
    }

    pub fn default_engine(&self) -> DbEngine {
        self.default_engine
    }

    fn adapter(&self, engine: DbEngine) -> DbResult<Arc<dyn DbAdminPort>> {
        self.adapters
            .get(&engine)
            .cloned()
            .ok_or(DbError::EngineNotConfigured { engine })
    }

    pub fn create_session(self: &Arc<Self>) -> DbShellSession {
        DbShellSession {
            service: Arc::clone(self),
            current_engine: self.default_engine,
        }
    }
}

#[derive(Clone)]
pub struct DbShellSession {
    service: Arc<DbShellService>,
    current_engine: DbEngine,
}

impl DbShellSession {
    fn guard_enabled(&self) -> DbResult<()> {
        if !self.service.is_enabled() {
            return Err(DbError::InvalidInput {
                message: "db-shell-service ist deaktiviert".to_string(),
            });
        }
        Ok(())
    }

    pub fn current_engine(&self) -> DbEngine {
        self.current_engine
    }

    pub fn available_engines(&self) -> Vec<DbEngine> {
        self.service.available_engines()
    }

    pub fn switch_engine(&mut self, engine: DbEngine) -> DbResult<()> {
        self.guard_enabled()?;
        if engine == self.current_engine {
            return Ok(());
        }
        // Ensure engine exists before switching.
        let _ = self.service.adapter(engine)?;
        self.current_engine = engine;
        Ok(())
    }

    pub async fn ping(&self) -> DbResult<()> {
        self.guard_enabled()?;
        let adapter = self.service.adapter(self.current_engine)?;
        adapter.ping().await
    }

    pub async fn simple_query(&self, statement: &str) -> DbResult<Vec<DbExecutionResult>> {
        self.guard_enabled()?;
        let adapter = self.service.adapter(self.current_engine)?;
        adapter.simple_query(statement).await
    }

    pub async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
        self.guard_enabled()?;
        let adapter = self.service.adapter(self.current_engine)?;
        adapter.list_tables().await
    }

    pub async fn describe_table(&self, table: &str) -> DbResult<DbTableSchema> {
        self.guard_enabled()?;
        let adapter = self.service.adapter(self.current_engine)?;
        adapter.describe_table(table).await
    }
}

#[derive(Clone)]
pub struct DbShellControl {
    service: Arc<DbShellService>,
    registry: Arc<ServiceRegistry>,
}

impl DbShellControl {
    pub fn new(service: Arc<DbShellService>, registry: Arc<ServiceRegistry>) -> Self {
        Self { service, registry }
    }
}

#[async_trait]
impl ManagedService for DbShellControl {
    fn id(&self) -> &'static str {
        "db-shell"
    }

    async fn start(self: Arc<Self>) -> anyhow::Result<bool> {
        if self.service.is_enabled() {
            return Ok(false);
        }
        self.service.set_enabled(true);
        self.registry.set_status(
            "db-shell",
            ServiceStatus::Active,
            Some("DB-Shell aktiviert".to_string()),
        );
        Ok(true)
    }

    async fn stop(self: Arc<Self>, _force: bool) -> anyhow::Result<bool> {
        if !self.service.is_enabled() {
            return Ok(false);
        }
        self.service.set_enabled(false);
        self.registry.set_status(
            "db-shell",
            ServiceStatus::Standby,
            Some("DB-Shell deaktiviert".to_string()),
        );
        Ok(true)
    }
}
