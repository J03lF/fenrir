use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::db::{
    DbAdminPort, DbEngine, DbError, DbExecutionResult, DbResult, DbTable, DbTableSchema,
};

#[derive(Clone)]
pub struct DbShellService {
    default_engine: DbEngine,
    adapters: Arc<BTreeMap<DbEngine, Arc<dyn DbAdminPort>>>,
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
        })
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
    pub fn current_engine(&self) -> DbEngine {
        self.current_engine
    }

    pub fn available_engines(&self) -> Vec<DbEngine> {
        self.service.available_engines()
    }

    pub fn switch_engine(&mut self, engine: DbEngine) -> DbResult<()> {
        if engine == self.current_engine {
            return Ok(());
        }
        // Ensure engine exists before switching.
        let _ = self.service.adapter(engine)?;
        self.current_engine = engine;
        Ok(())
    }

    pub async fn ping(&self) -> DbResult<()> {
        let adapter = self.service.adapter(self.current_engine)?;
        adapter.ping().await
    }

    pub async fn simple_query(&self, statement: &str) -> DbResult<Vec<DbExecutionResult>> {
        let adapter = self.service.adapter(self.current_engine)?;
        adapter.simple_query(statement).await
    }

    pub async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
        let adapter = self.service.adapter(self.current_engine)?;
        adapter.list_tables().await
    }

    pub async fn describe_table(&self, table: &str) -> DbResult<DbTableSchema> {
        let adapter = self.service.adapter(self.current_engine)?;
        adapter.describe_table(table).await
    }
}
