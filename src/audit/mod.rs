use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    #[serde(with = "serde_time")]
    pub timestamp: SystemTime,
    pub actor: AuditActor,
    pub action: String,
    pub target: String,
    pub outcome: AuditOutcome,
    pub redactions: Vec<String>,
    pub metadata: AuditMetadata,
}

impl AuditEvent {
    pub fn builder() -> AuditEventBuilder {
        AuditEventBuilder::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AuditMetadata {
    entries: Vec<(String, String)>,
}

impl AuditMetadata {
    pub fn insert(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push((key.into(), value.into()));
        self
    }

    pub fn as_slice(&self) -> &[(String, String)] {
        &self.entries
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditActor {
    System,
    User { user_id: String, role: String },
}

impl AuditActor {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditActor::System => "system",
            AuditActor::User { .. } => "user",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditOutcome {
    Success,
    Failure,
    Denied,
}

impl fmt::Display for AuditOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuditOutcome::Success => f.write_str("success"),
            AuditOutcome::Failure => f.write_str("failure"),
            AuditOutcome::Denied => f.write_str("denied"),
        }
    }
}

impl Default for AuditOutcome {
    fn default() -> Self {
        AuditOutcome::Success
    }
}

#[derive(Default)]
pub struct AuditEventBuilder {
    actor: Option<AuditActor>,
    action: Option<String>,
    target: Option<String>,
    outcome: AuditOutcome,
    redactions: Vec<String>,
    metadata: AuditMetadata,
}

impl AuditEventBuilder {
    pub fn actor(mut self, actor: AuditActor) -> Self {
        self.actor = Some(actor);
        self
    }

    pub fn action(mut self, action: impl Into<String>) -> Self {
        self.action = Some(action.into());
        self
    }

    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    pub fn outcome(mut self, outcome: AuditOutcome) -> Self {
        self.outcome = outcome;
        self
    }

    pub fn redact(mut self, field: impl Into<String>) -> Self {
        self.redactions.push(field.into());
        self
    }

    pub fn metadata(mut self, metadata: AuditMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn build(self) -> Result<AuditEvent, AuditError> {
        Ok(AuditEvent {
            timestamp: SystemTime::now(),
            actor: self.actor.ok_or_else(|| {
                AuditError::Validation("Audit-Actor muss gesetzt werden".to_string())
            })?,
            action: self
                .action
                .ok_or_else(|| AuditError::Validation("Action fehlt".to_string()))?,
            target: self
                .target
                .ok_or_else(|| AuditError::Validation("Target fehlt".to_string()))?,
            outcome: self.outcome,
            redactions: self.redactions,
            metadata: self.metadata,
        })
    }
}

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
    ) -> Self {
        let initial_events = load_persisted_events(&path, retention, capacity);
        Self {
            capacity,
            events: RwLock::new(initial_events),
            persistence: Some(PersistenceConfig::new(path, retention, persist_interval)),
        }
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
                .map_err(|_| AuditError::Storage("Audit-Log gesperrt".to_string()))?;
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
            .map_err(|_| AuditError::Storage("Audit-Log gesperrt".to_string()))?;
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

struct PersistenceConfig {
    path: PathBuf,
    retention: Duration,
    persist_interval: Duration,
    last_persist: Mutex<Instant>,
}

impl PersistenceConfig {
    fn new(path: PathBuf, retention: Duration, persist_interval: Duration) -> Self {
        let initial = Instant::now()
            .checked_sub(persist_interval)
            .unwrap_or_else(Instant::now);
        Self {
            path,
            retention,
            persist_interval,
            last_persist: Mutex::new(initial),
        }
    }

    fn should_persist(&self) -> bool {
        if let Ok(mut guard) = self.last_persist.lock() {
            if guard.elapsed() >= self.persist_interval {
                *guard = Instant::now();
                return true;
            }
        }
        false
    }

    fn persist(&self, events: Vec<AuditEvent>) {
        let payload = AuditHistoryFile {
            version: AUDIT_HISTORY_VERSION,
            events,
        };
        match serde_json::to_vec(&payload) {
            Ok(data) => {
                if let Some(parent) = self.path.parent() {
                    if let Err(err) = fs::create_dir_all(parent) {
                        tracing::debug!(error = %err, path = ?parent, "audit history directory creation failed");
                        return;
                    }
                }
                if let Err(err) = fs::write(&self.path, data) {
                    tracing::debug!(error = %err, path = ?self.path, "audit history persist failed");
                }
            }
            Err(err) => {
                tracing::debug!(error = %err, "audit history serialization failed");
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct AuditHistoryFile {
    version: u8,
    events: Vec<AuditEvent>,
}

const AUDIT_HISTORY_VERSION: u8 = 1;

fn load_persisted_events(
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
                tracing::debug!(error = %err, path = ?path, "audit history parse failed");
                VecDeque::new()
            }
        },
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(error = %err, path = ?path, "audit history read failed");
            }
            VecDeque::new()
        }
    }
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum AuditError {
    #[error("Audit Validation: {0}")]
    Validation(String),
    #[error("Audit Storage: {0}")]
    Storage(String),
}

mod serde_time {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let datetime: OffsetDateTime = (*time).into();
        let formatted = datetime
            .format(&Rfc3339)
            .map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&formatted)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let datetime = OffsetDateTime::parse(&raw, &Rfc3339).map_err(serde::de::Error::custom)?;
        Ok(SystemTime::from(datetime))
    }
}
