use std::sync::Arc;

use fenrir::config;
use fenrir::domain::db::DbEngine;
use fenrir::infra::db::manager;
use fenrir::services::db_shell::DbShellService;
use fenrir::services::db_schema::DatabaseBlueprint;

// This test expects a running postgres reachable via FENRIR_DB_POSTGRES_URI.
// It creates a temp table with PK/FK to verify detection.
#[tokio::test]
async fn snapshot_postgres_schema_detects_pk_fk() {
    // Load config to get postgres uri (env-driven)
    let cfg = config::load().expect("config load");
    if cfg.db.default_engine != "postgres" {
        eprintln!("Skipping: db.default_engine != postgres");
        return;
    }
    let adapters = manager::build_adapters(&cfg, None).expect("build adapters");
    let db_shell = Arc::new(DbShellService::new(DbEngine::Postgres, adapters).unwrap());
    let mut session = db_shell.create_session();

    // prepare schema
    session
        .simple_query("DROP TABLE IF EXISTS child; DROP TABLE IF EXISTS parent;")
        .await
        .ok();
    session
        .simple_query("CREATE TABLE parent (id SERIAL PRIMARY KEY, name TEXT);")
        .await
        .expect("create parent");
    session
        .simple_query(
            "CREATE TABLE child (id SERIAL PRIMARY KEY, parent_id INT REFERENCES parent(id));",
        )
        .await
        .expect("create child");

    let blueprint =
        DatabaseBlueprint::snapshot(Arc::clone(&db_shell), Some(DbEngine::Postgres))
            .await
            .expect("snapshot");

    let parent = blueprint.tables.iter().find(|t| t.name == "parent").unwrap();
    assert!(parent
        .columns
        .iter()
        .any(|c| c.name == "id" && c.is_primary));

    let child = blueprint.tables.iter().find(|t| t.name == "child").unwrap();
    assert!(child
        .columns
        .iter()
        .any(|c| c.name == "parent_id" && c.references.as_deref() == Some("parent(id)")));
}

