use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use tokio::sync::broadcast;

use super::types::{ServiceDescriptorOwned, ServiceSnapshot, ServiceStatus};
use crate::utils::messages::services::registry::logs as registry_logs;

struct ServiceRecord {
    descriptor: ServiceDescriptorOwned,
    status: ServiceStatus,
    since: SystemTime,
    note: Option<String>,
}

#[derive(Clone)]
pub struct ServiceRegistry {
    inner: Arc<RwLock<BTreeMap<String, ServiceRecord>>>,
    events: broadcast::Sender<ServiceSnapshot>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceRegistry {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            inner: Arc::new(RwLock::new(BTreeMap::new())),
            events,
        }
    }

    pub fn register(
        &self,
        descriptor: impl Into<ServiceDescriptorOwned>,
        status: ServiceStatus,
        note: impl Into<Option<String>>,
    ) {
        let descriptor_owned = descriptor.into();
        let id = descriptor_owned.id().to_string();
        let record = ServiceRecord {
            descriptor: descriptor_owned,
            status,
            since: SystemTime::now(),
            note: note.into(),
        };
        let snapshot = if let Ok(mut guard) = self.inner.write() {
            guard.insert(id.clone(), record);
            guard.get(&id).map(snapshot_from_record)
        } else {
            tracing::error!(service_id = id, "{}", registry_logs::LOCK_POISONED);
            None
        };
        if let Some(snapshot) = snapshot {
            let _ = self.events.send(snapshot);
        }
    }

    pub fn set_status(&self, id: &str, status: ServiceStatus, note: impl Into<Option<String>>) {
        let snapshot = match self.inner.write() {
            Ok(mut guard) => {
                if let Some(record) = guard.get_mut(id) {
                    record.status = status;
                    record.note = note.into();
                    record.since = SystemTime::now();
                    Some(snapshot_from_record(record))
                } else {
                    tracing::warn!(service_id = id, "{}", registry_logs::UNKNOWN_UPDATE);
                    None
                }
            }
            Err(_) => {
                tracing::error!(service_id = id, "{}", registry_logs::LOCK_POISONED);
                None
            }
        };
        if let Some(snapshot) = snapshot {
            let _ = self.events.send(snapshot);
        }
    }

    pub fn update_note(&self, id: &str, note: impl Into<Option<String>>) {
        let snapshot = if let Ok(mut guard) = self.inner.write() {
            if let Some(record) = guard.get_mut(id) {
                record.note = note.into();
                Some(snapshot_from_record(record))
            } else {
                None
            }
        } else {
            None
        };
        if let Some(snapshot) = snapshot {
            let _ = self.events.send(snapshot);
        }
    }

    pub fn unregister(&self, id: &str) {
        if let Ok(mut guard) = self.inner.write() {
            guard.remove(id);
        }
    }

    pub fn unregister_prefixed(&self, prefix: &str) {
        if let Ok(mut guard) = self.inner.write() {
            let ids: Vec<String> = guard
                .keys()
                .filter(|key| key.starts_with(prefix))
                .cloned()
                .collect();
            for id in ids {
                guard.remove(&id);
            }
        }
    }

    pub fn snapshot(&self) -> Vec<ServiceSnapshot> {
        match self.inner.read() {
            Ok(guard) => guard.values().map(snapshot_from_record).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<ServiceSnapshot> {
        match self.inner.read() {
            Ok(guard) => guard.get(id).map(snapshot_from_record),
            Err(_) => None,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServiceSnapshot> {
        self.events.subscribe()
    }
}

fn snapshot_from_record(record: &ServiceRecord) -> ServiceSnapshot {
    ServiceSnapshot {
        descriptor: record.descriptor.clone(),
        status: record.status,
        since: record.since,
        note: record.note.clone(),
    }
}

#[cfg(test)]
mod tests {
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
}
