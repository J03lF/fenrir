use std::sync::Arc;
use std::time::Duration;

use fenrir::config;
use fenrir::infra::db::runtime::DbRuntimeSupervisor;
use fenrir::services::diagnostics::ServiceDiagnostics;
use fenrir::services::managed::block_on_managed;
use tokio::time::sleep;

#[tokio::test]
async fn db_runtime_status_and_logs() {
    let cfg = config::load().expect("config load");
    if cfg.db.runtime.mode != config::DbRuntimeMode::Embedded {
        eprintln!("Skipping: db.runtime.mode != embedded");
        return;
    }
    let rt = DbRuntimeSupervisor::new(
        cfg.db.runtime.mode,
        cfg.db.runtime.embedded.engine,
        cfg.db.runtime.embedded.postgres.port_range,
        cfg.db.runtime.embedded.postgres.binary_path.clone(),
        std::path::PathBuf::from("runtime/db-test-int"),
        Arc::new(fenrir::security::manager::SecurityManager::new(
            &cfg.security,
            Arc::new(fenrir::services::app::AppServices::new_dummy_audit()),
        )
        .unwrap()),
        Arc::new(ServiceDiagnostics::new()),
    );

    let started = block_on_managed(rt.clone().start()).unwrap();
    if started {
        sleep(Duration::from_millis(200)).await;
        let status = rt.status_snapshot().unwrap();
        assert!(status.running);
        let logs = rt.logs(10);
        assert!(!logs.is_empty());
        block_on_managed(rt.clone().stop(false)).unwrap();
    }
}

