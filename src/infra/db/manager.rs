use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;

use crate::config::{AppConfig, DbConnectionSettings, DbRuntimeMode, EmbeddedEngineKind};
use crate::domain::db::{DbAdminPort, DbEngine};
use crate::utils::messages::infra::db as infra_db_messages;

use super::adapters;

#[derive(Debug, Clone)]
pub struct RuntimeDbOverride {
    pub engine: DbEngine,
    pub uri: String,
}

pub fn build_adapters(
    cfg: &AppConfig,
    runtime_override: Option<RuntimeDbOverride>,
) -> Result<BTreeMap<DbEngine, Arc<dyn DbAdminPort>>> {
    let mut map: BTreeMap<DbEngine, Arc<dyn DbAdminPort>> = BTreeMap::new();

    // Track which engines are handled by embedded runtime (to skip external config).
    let embedded_engine: Option<DbEngine> = if cfg.db.runtime.mode == DbRuntimeMode::Embedded {
        Some(match cfg.db.runtime.embedded.engine {
            EmbeddedEngineKind::Sqlite => DbEngine::Sqlite,
            EmbeddedEngineKind::Postgres => DbEngine::Postgres,
        })
    } else {
        None
    };

    // Runtime override from early-started supervisor takes highest priority.
    if let Some(override_cfg) = runtime_override {
        match override_cfg.engine {
            DbEngine::Sqlite => {
                let adapter = adapters::sqlite::adapter::SqliteAdapter::new(&override_cfg.uri)?;
                map.insert(DbEngine::Sqlite, Arc::new(adapter));
            }
            DbEngine::Postgres => {
                let adapter =
                    adapters::postgres::PostgresAdapter::new(&override_cfg.uri, None, None)?;
                map.insert(DbEngine::Postgres, Arc::new(adapter));
            }
            other => {
                tracing::warn!(engine = %other, "runtime override not supported for engine");
            }
        }
    }

    // Embedded sqlite fallback (if no runtime override was provided).
    if embedded_engine == Some(DbEngine::Sqlite) && !map.contains_key(&DbEngine::Sqlite) {
        let uri = cfg.db.runtime.embedded.sqlite.file_path.clone();
        let adapter = build_sqlite(&DbConnectionSettings {
            uri,
            pool: crate::config::DbPoolSettings::default(),
        })?;
        map.insert(DbEngine::Sqlite, Arc::new(adapter));
    }

    // External postgres (only if NOT using embedded postgres).
    if embedded_engine != Some(DbEngine::Postgres) {
        if let Some(settings) = cfg.db.connections.postgres.as_ref() {
            let adapter = build_postgres(settings)?;
            map.insert(DbEngine::Postgres, Arc::new(adapter));
        }
    }

    if let Some(settings) = cfg.db.connections.mysql.as_ref() {
        warn_unimplemented(DbEngine::Mysql, settings);
    }

    // External sqlite (only if NOT using embedded sqlite).
    if embedded_engine != Some(DbEngine::Sqlite) {
        if let Some(settings) = cfg.db.connections.sqlite.as_ref() {
            let adapter = build_sqlite(settings)?;
            map.insert(DbEngine::Sqlite, Arc::new(adapter));
        }
    }

    if let Some(settings) = cfg.db.connections.mongodb.as_ref() {
        warn_unimplemented(DbEngine::Mongodb, settings);
    }

    Ok(map)
}

fn build_postgres(settings: &DbConnectionSettings) -> Result<adapters::postgres::PostgresAdapter> {
    let uri = settings.resolve_uri("db.connections.postgres.uri")?;
    adapters::postgres::PostgresAdapter::new(&uri, settings.pool_max(), settings.pool_timeout())
}

fn build_sqlite(
    settings: &DbConnectionSettings,
) -> Result<adapters::sqlite::adapter::SqliteAdapter> {
    let uri = settings.resolve_uri("db.connections.sqlite.uri")?;
    adapters::sqlite::adapter::SqliteAdapter::new(&uri)
}

fn warn_unimplemented(engine: DbEngine, _settings: &DbConnectionSettings) {
    tracing::warn!(
        engine = %engine,
        "{}",
        infra_db_messages::manager::ADAPTER_UNIMPLEMENTED
    );
}
