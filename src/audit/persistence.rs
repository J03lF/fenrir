use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::error::AuditError;
use super::event::AuditEvent;
use crate::utils::messages::audit::persistence as audit_messages;

const AUDIT_HISTORY_VERSION: u8 = 1;

pub(super) struct PersistenceConfig {
    pub(super) path: PathBuf,
    pub(super) retention: Duration,
    persist_interval: Duration,
    last_persist: Mutex<Instant>,
}

impl PersistenceConfig {
    pub fn new(
        path: PathBuf,
        retention: Duration,
        persist_interval: Duration,
    ) -> Result<Self, AuditError> {
        let probe = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| AuditError::Storage(audit_messages::path_not_writable(&path, &err)))?;
        drop(probe);

        let initial = Instant::now()
            .checked_sub(persist_interval)
            .unwrap_or_else(Instant::now);
        Ok(Self {
            path,
            retention,
            persist_interval,
            last_persist: Mutex::new(initial),
        })
    }

    pub fn should_persist(&self) -> bool {
        if let Ok(mut guard) = self.last_persist.lock() {
            if guard.elapsed() >= self.persist_interval {
                *guard = Instant::now();
                return true;
            }
        }
        false
    }

    pub fn persist(&self, events: Vec<AuditEvent>) {
        let payload = AuditHistoryFile {
            version: AUDIT_HISTORY_VERSION,
            events,
        };
        match serde_json::to_vec(&payload) {
            Ok(data) => {
                if let Some(parent) = self.path.parent() {
                    if let Err(err) = fs::create_dir_all(parent) {
                        tracing::debug!(
                            error = %err,
                            path = ?parent,
                            "{}",
                            audit_messages::HISTORY_DIR_CREATE_FAILED
                        );
                        return;
                    }
                }
                if let Err(err) = fs::write(&self.path, data) {
                    tracing::debug!(
                        error = %err,
                        path = ?self.path,
                        "{}",
                        audit_messages::HISTORY_PERSIST_FAILED
                    );
                }
            }
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "{}",
                    audit_messages::HISTORY_SERIALIZATION_FAILED
                );
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct AuditHistoryFile {
    version: u8,
    events: Vec<AuditEvent>,
}

pub(super) fn load_persisted_events(
    path: &PathBuf,
    retention: Duration,
    capacity: usize,
) -> VecDeque<AuditEvent> {
    if capacity == 0 {
        return VecDeque::new();
    }

    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<AuditHistoryFile>(&bytes) {
            Ok(file) => {
                let cutoff = SystemTime::now()
                    .checked_sub(retention)
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                let mut events: Vec<AuditEvent> = file
                    .events
                    .into_iter()
                    .filter(|event| event.timestamp >= cutoff)
                    .collect();
                events.sort_by_key(|event| {
                    event
                        .timestamp
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_else(|_| Duration::from_secs(0))
                });
                if events.len() > capacity {
                    events = events.split_off(events.len() - capacity);
                }
                VecDeque::from(events)
            }
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    path = ?path,
                    "{}",
                    audit_messages::HISTORY_PARSE_FAILED
                );
                VecDeque::new()
            }
        },
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(
                    error = %err,
                    path = ?path,
                    "{}",
                    audit_messages::HISTORY_READ_FAILED
                );
            }
            VecDeque::new()
        }
    }
}
