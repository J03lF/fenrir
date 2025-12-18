use super::*;

#[test]
fn service_resource_reported_flag() {
    let empty = ServiceResourceEntry::default();
    let snapshot = empty.snapshot("svc-empty");
    assert!(!snapshot.reported);
    assert!(snapshot.cpu_percent.is_none());

    let entry = ServiceResourceEntry {
        cpu_percent: Some(12.5),
        ..Default::default()
    };
    let snapshot = entry.snapshot("svc-cpu");
    assert!(snapshot.reported);
    assert_eq!(snapshot.cpu_percent, Some(12.5));
}

#[test]
fn update_service_resource_clamps_cpu() {
    let state = TelemetryState::new(TelemetryConfig {
        metrics_enabled: true,
        health_enabled: true,
        history_retention: Duration::from_secs(DEFAULT_HISTORY_RETENTION_SECS),
        history_persist_interval: Duration::from_secs(DEFAULT_HISTORY_PERSIST_INTERVAL_SECS),
        history_sample_interval: Duration::from_secs(DEFAULT_HISTORY_SAMPLE_INTERVAL_SECS),
    });
    state.update_service_resource(
        "svc",
        ServiceResourceSample {
            cpu_percent: Some(180.0),
            memory_bytes: Some(42),
            memory_peak_bytes: None,
        },
    );
    let resources = state.service_resources();
    assert_eq!(resources.len(), 1);
    let svc = &resources[0];
    assert_eq!(svc.id, "svc");
    assert_eq!(svc.cpu_percent, Some(100.0));
    assert_eq!(svc.memory_bytes, Some(42));
    assert!(svc.reported);
}
