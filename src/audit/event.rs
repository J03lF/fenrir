use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::SystemTime;

use super::error::AuditError;
use super::serde_time;
use crate::utils::messages::audit::{actors, builder, outcomes};

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
            AuditActor::System => actors::SYSTEM,
            AuditActor::User { .. } => actors::USER,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AuditOutcome {
    #[default]
    Success,
    Failure,
    Denied,
}

impl fmt::Display for AuditOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuditOutcome::Success => f.write_str(outcomes::SUCCESS),
            AuditOutcome::Failure => f.write_str(outcomes::FAILURE),
            AuditOutcome::Denied => f.write_str(outcomes::DENIED),
        }
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
            actor: self
                .actor
                .ok_or_else(|| AuditError::Validation(builder::MISSING_ACTOR.to_string()))?,
            action: self
                .action
                .ok_or_else(|| AuditError::Validation(builder::MISSING_ACTION.to_string()))?,
            target: self
                .target
                .ok_or_else(|| AuditError::Validation(builder::MISSING_TARGET.to_string()))?,
            outcome: self.outcome,
            redactions: self.redactions,
            metadata: self.metadata,
        })
    }
}
