use std::collections::VecDeque;
use std::fmt;
use std::sync::RwLock;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEvent {
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

#[derive(Clone, Debug, PartialEq, Eq, Default)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
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
}

impl InMemoryAuditLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            events: RwLock::new(VecDeque::with_capacity(capacity)),
        }
    }
}

impl AuditLog for InMemoryAuditLog {
    fn append(&self, event: AuditEvent) -> Result<(), AuditError> {
        if self.capacity == 0 {
            return Ok(());
        }
        let mut guard = self
            .events
            .write()
            .map_err(|_| AuditError::Storage("Audit-Log gesperrt".to_string()))?;
        if guard.len() == self.capacity {
            guard.pop_front();
        }
        guard.push_back(event);
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

#[derive(thiserror::Error, Debug, Clone)]
pub enum AuditError {
    #[error("Audit Validation: {0}")]
    Validation(String),
    #[error("Audit Storage: {0}")]
    Storage(String),
}
