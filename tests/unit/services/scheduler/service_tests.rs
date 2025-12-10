use super::*;
use crate::services::{ServiceDescriptor, ServiceKind, ServiceRegistry, ServiceStatus};

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
    let scheduler = SchedulerService::new(Arc::clone(&registry));
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
