use std::sync::Arc;
use tempfile::NamedTempFile;

use fenrir::domain::db::{DbEngine, DbValue};
use fenrir::infra::db::adapters::sqlite::adapter::SqliteAdapter;
use fenrir::services::db_shell::DbShellService;
use fenrir::services::db_schema::DatabaseBlueprint;

#[tokio::test]
async fn export_sqlite_schema_to_staruml() {
    // prepare sqlite file
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    let adapter = SqliteAdapter::new(&path).unwrap();
    let mut adapters = std::collections::BTreeMap::new();
    adapters.insert(DbEngine::Sqlite, Arc::new(adapter) as Arc<_>);
    let db_shell = Arc::new(DbShellService::new(DbEngine::Sqlite, adapters).unwrap());

    // create a table
    let session = db_shell.create_session();
    session
        .simple_query("CREATE TABLE test_items (id INTEGER PRIMARY KEY, name TEXT)")
        .await
        .unwrap();

    // snapshot and export
    let blueprint = DatabaseBlueprint::snapshot(Arc::clone(&db_shell), Some(DbEngine::Sqlite))
        .await
        .unwrap();
    assert_eq!(blueprint.engine, DbEngine::Sqlite);
    assert!(blueprint.tables.iter().any(|t| t.name == "test_items"));

    let export_path = tmp.path().with_extension("mdj");
    blueprint.export_staruml(&export_path).await.unwrap();
    assert!(export_path.exists());
}

