use super::*;
use crate::services::types::{ServiceDescriptor, ServiceKind};

#[test]
fn broadcasts_on_register_and_update() {
    let registry = ServiceRegistry::new();
    let mut receiver = registry.subscribe();
    let descriptor =
        ServiceDescriptor::new("test-service", "Test Service", "Test", ServiceKind::Other);
    registry.register(descriptor, ServiceStatus::Starting, None::<String>);
    let event = receiver.try_recv().expect("expected register event");
    assert_eq!(event.descriptor.id, "test-service");
    assert_eq!(event.status, ServiceStatus::Starting);

    registry.set_status(
        "test-service",
        ServiceStatus::Active,
        Some("ok".to_string()),
    );
    let event = receiver.try_recv().expect("expected status update");
    assert_eq!(event.status, ServiceStatus::Active);
    assert_eq!(event.note.as_deref(), Some("ok"));
}
