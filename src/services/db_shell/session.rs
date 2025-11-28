use std::sync::Arc;

use crate::domain::db::{DbEngine, DbError, DbExecutionResult, DbResult, DbTable, DbTableSchema};
use crate::utils::messages::services::db_shell::errors as db_shell_errors;

use super::DbShellService;

#[derive(Clone)]
pub struct DbShellSession {
    service: Arc<DbShellService>,
    current_engine: DbEngine,
}

impl DbShellSession {
    pub(super) fn new(service: Arc<DbShellService>) -> Self {
        Self {
            current_engine: service.default_engine(),
            service,
        }
    }

    fn guard_enabled(&self) -> DbResult<()> {
        if !self.service.is_enabled() {
            return Err(DbError::InvalidInput {
                message: db_shell_errors::SERVICE_DISABLED.to_string(),
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
