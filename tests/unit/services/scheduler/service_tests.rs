use super::*;
use crate::services::{
    ServiceDescriptor, ServiceDiagnostics, ServiceKind, ServiceRegistry, ServiceStatus,
};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn schedules_and_cancels_jobs() {
    let registry = Arc::new(ServiceRegistry::new());
    registry.register(
        ServiceDescriptor::new(
            "scheduler",
            "Scheduler",
            "test scheduler",
            ServiceKind::BackgroundJob,
        ),
        ServiceStatus::Starting,
        None,
    );
    let diagnostics = Arc::new(ServiceDiagnostics::new());
    let state_dir = std::env::temp_dir().join(format!("fenrir-scheduler-test-{}", Uuid::new_v4()));
    let scheduler =
        SchedulerService::new(Arc::clone(&registry), Arc::clone(&diagnostics), state_dir);
    assert!(scheduler.start());

    let spec = ScheduledJobSpec {
        id: "heartbeat".to_string(),
        interval: Duration::from_millis(10),
        initial_delay: None,
        description: "heartbeat".to_string(),
    };

    scheduler
        .schedule_fixed_rate(spec, || async { Ok(()) })
        .expect("schedule job");

    tokio::time::sleep(Duration::from_millis(25)).await;

    assert_eq!(scheduler.jobs().len(), 1);
    assert!(scheduler.cancel_job("heartbeat"));
    assert!(scheduler.stop());
}

#[tokio::test]
async fn pauses_and_resumes_jobs() {
    let registry = Arc::new(ServiceRegistry::new());
    registry.register(
        ServiceDescriptor::new(
            "scheduler",
            "Scheduler",
            "test scheduler",
            ServiceKind::BackgroundJob,
        ),
        ServiceStatus::Starting,
        None,
    );
    let diagnostics = Arc::new(ServiceDiagnostics::new());
    let state_dir = std::env::temp_dir().join(format!("fenrir-scheduler-test-{}", Uuid::new_v4()));
    let scheduler =
        SchedulerService::new(Arc::clone(&registry), Arc::clone(&diagnostics), state_dir);
    assert!(scheduler.start());

    let spec = ScheduledJobSpec {
        id: "heartbeat".to_string(),
        interval: Duration::from_millis(10),
        initial_delay: None,
        description: "heartbeat".to_string(),
    };

    scheduler
        .schedule_fixed_rate(spec, || async { Ok(()) })
        .expect("schedule job");

    assert!(matches!(
        scheduler.pause_job("heartbeat"),
        Ok(JobControlOutcome::Paused)
    ));
    let snapshot = scheduler.job("heartbeat").expect("job snapshot");
    assert!(snapshot.paused);

    assert!(matches!(
        scheduler.resume_job("heartbeat"),
        Ok(JobControlOutcome::Resumed)
    ));
    let snapshot = scheduler.job("heartbeat").expect("job snapshot");
    assert!(!snapshot.paused);

    assert!(scheduler.stop());
}
