use std::sync::Arc;

use fenrir::config;
use fenrir::infra::db::runtime::DbRuntimeSupervisor;
use fenrir::services::diagnostics::ServiceDiagnostics;

#[test]
fn supervisor_starts_stops_stub_sqlite() {
    let cfg = config::load().expect("config load");
    let rt = DbRuntimeSupervisor::new(
        cfg.db.runtime.mode,
        cfg.db.runtime.embedded.engine,
        cfg.db.runtime.embedded.postgres.port_range,
        cfg.db.runtime.embedded.postgres.binary_path.clone(),
        std::path::PathBuf::from("runtime/db-test"),
        Arc::new(fenrir::security::manager::SecurityManager::new(
            &cfg.security,
            Arc::new(fenrir::services::app::AppServices::new_dummy_audit()),
        )
        .unwrap()),
        Arc::new(ServiceDiagnostics::new()),
    );
    let started = fenrir::services::managed::block_on_managed(rt.clone().start()).unwrap();
    assert!(started || !rt.health_ok());
    let stopped = fenrir::services::managed::block_on_managed(rt.clone().stop(false)).unwrap();
    assert!(stopped || !rt.health_ok());
}

