use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{Duration, SystemTime};

use super::error::AuditError;
use super::event::AuditEvent;
use super::persistence::{load_persisted_events, PersistenceConfig};
use crate::utils::messages::audit::errors as audit_errors;

pub trait AuditLog: Send + Sync {
    fn append(&self, event: AuditEvent) -> Result<(), AuditError>;
    fn recent(&self, limit: usize) -> Result<Vec<AuditEvent>, AuditError>;
}

pub struct InMemoryAuditLog {
    capacity: usize,
    events: RwLock<VecDeque<AuditEvent>>,
    persistence: Option<PersistenceConfig>,
}

impl InMemoryAuditLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            events: RwLock::new(VecDeque::with_capacity(capacity)),
            persistence: None,
        }
    }

    pub fn with_persistence(
        capacity: usize,
        path: PathBuf,
        retention: Duration,
        persist_interval: Duration,
    ) -> Result<Self, AuditError> {
        let initial_events = load_persisted_events(&path, retention, capacity);
        let persistence = PersistenceConfig::new(path, retention, persist_interval)?;
        Ok(Self {
            capacity,
            events: RwLock::new(initial_events),
            persistence: Some(persistence),
        })
    }

    fn prune_retention(&self, events: &mut VecDeque<AuditEvent>) {
        if let Some(persistence) = &self.persistence {
            let cutoff = SystemTime::now()
                .checked_sub(persistence.retention)
                .unwrap_or(SystemTime::UNIX_EPOCH);
            while let Some(front) = events.front() {
                if front.timestamp < cutoff {
                    events.pop_front();
                } else {
                    break;
                }
            }
        }
        while events.len() > self.capacity {
            events.pop_front();
        }
    }
}

impl AuditLog for InMemoryAuditLog {
    fn append(&self, event: AuditEvent) -> Result<(), AuditError> {
        if self.capacity == 0 {
            return Ok(());
        }
        let mut snapshot_to_persist: Option<Vec<AuditEvent>> = None;
        {
            let mut guard = self
                .events
                .write()
                .map_err(|_| AuditError::Storage(audit_errors::LOG_LOCKED.to_string()))?;
            guard.push_back(event);
            self.prune_retention(&mut guard);
            if let Some(persistence) = &self.persistence {
                let should_flush = persistence.should_persist();
                if should_flush || snapshot_to_persist.is_none() {
                    snapshot_to_persist = Some(guard.iter().cloned().collect());
                }
            }
        }
        if let (Some(persistence), Some(snapshot)) = (&self.persistence, snapshot_to_persist) {
            persistence.persist(snapshot);
        }
        Ok(())
    }

    fn recent(&self, limit: usize) -> Result<Vec<AuditEvent>, AuditError> {
        let guard = self
            .events
            .read()
            .map_err(|_| AuditError::Storage(audit_errors::LOG_LOCKED.to_string()))?;
        let count = guard.len().min(limit);
        Ok(guard.iter().rev().take(count).cloned().collect())
    }
}

impl Drop for InMemoryAuditLog {
    fn drop(&mut self) {
        if let Some(persistence) = &self.persistence {
            if let Ok(events) = self.events.write() {
                let snapshot: Vec<AuditEvent> = events.iter().cloned().collect();
                persistence.persist(snapshot);
            }
        }
    }
}
