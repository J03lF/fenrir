use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::domain::db::{DbAdminPort, DbEngine, DbError, DbResult};

use super::DbShellSession;

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

    pub(super) fn adapter(&self, engine: DbEngine) -> DbResult<Arc<dyn DbAdminPort>> {
        self.adapters
            .get(&engine)
            .cloned()
            .ok_or(DbError::EngineNotConfigured { engine })
    }

    pub fn create_session(self: &Arc<Self>) -> DbShellSession {
        DbShellSession::new(Arc::clone(self))
    }
}
