use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;

use crate::config::{AppConfig, DbConnectionSettings};
use crate::domain::db::{DbAdminPort, DbEngine};

use super::adapters;

pub fn build_adapters(cfg: &AppConfig) -> Result<BTreeMap<DbEngine, Arc<dyn DbAdminPort>>> {
    let mut map: BTreeMap<DbEngine, Arc<dyn DbAdminPort>> = BTreeMap::new();

    if let Some(settings) = cfg.db.connections.postgres.as_ref() {
        let adapter = build_postgres(settings)?;
        map.insert(DbEngine::Postgres, Arc::new(adapter));
    }
    if let Some(settings) = cfg.db.connections.mysql.as_ref() {
        warn_unimplemented(DbEngine::Mysql, settings);
    }
    if let Some(settings) = cfg.db.connections.sqlite.as_ref() {
        warn_unimplemented(DbEngine::Sqlite, settings);
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

fn warn_unimplemented(engine: DbEngine, _settings: &DbConnectionSettings) {
    tracing::warn!(engine = %engine, "DB-Adapter noch nicht implementiert");
}
