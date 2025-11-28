use std::time::SystemTime;

use super::managed::{ServiceControlError, ServiceControlOutcome};
use crate::security::auth::Role;
use crate::utils::messages::services::types::{
    action_kind, kind as kind_messages, status as status_messages, tag as tag_messages,
};

#[derive(Debug)]
pub struct ServiceActionReport {
    pub id: String,
    pub result: Result<ServiceControlOutcome, ServiceControlError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceActionKind {
    Start,
    Stop,
    Restart,
}

impl ServiceActionKind {
    pub fn as_str(self) -> &'static str {
        self.verb()
    }

    pub fn verb(self) -> &'static str {
        match self {
            ServiceActionKind::Start => action_kind::START,
            ServiceActionKind::Stop => action_kind::STOP,
            ServiceActionKind::Restart => action_kind::RESTART,
        }
    }

    pub fn required_role(self) -> Role {
        match self {
            ServiceActionKind::Start => Role::Operator,
            ServiceActionKind::Stop => Role::Operator,
            ServiceActionKind::Restart => Role::Admin,
        }
    }

    pub fn supports_force(self) -> bool {
        matches!(self, ServiceActionKind::Stop | ServiceActionKind::Restart)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    Infrastructure,
    Transport,
    BackgroundJob,
    Cli,
    Security,
    Storage,
    Other,
}

impl ServiceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceKind::Infrastructure => kind_messages::INFRASTRUCTURE,
            ServiceKind::Transport => kind_messages::TRANSPORT,
            ServiceKind::BackgroundJob => kind_messages::BACKGROUND_JOB,
            ServiceKind::Cli => kind_messages::CLI,
            ServiceKind::Security => kind_messages::SECURITY,
            ServiceKind::Storage => kind_messages::STORAGE,
            ServiceKind::Other => kind_messages::OTHER,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServiceTag {
    Core,
    Platform,
    Auxiliary,
}

impl ServiceTag {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceTag::Core => tag_messages::CORE,
            ServiceTag::Platform => tag_messages::PLATFORM,
            ServiceTag::Auxiliary => tag_messages::AUXILIARY,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceStatus {
    Starting,
    Active,
    Degraded,
    Failed,
    Standby,
    Stopped,
}

impl ServiceStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ServiceStatus::Starting => status_messages::STARTING,
            ServiceStatus::Active => status_messages::ACTIVE,
            ServiceStatus::Degraded => status_messages::DEGRADED,
            ServiceStatus::Failed => status_messages::FAILED,
            ServiceStatus::Standby => status_messages::STANDBY,
            ServiceStatus::Stopped => status_messages::STOPPED,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ServiceDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub kind: ServiceKind,
    pub critical: bool,
    pub tags: &'static [ServiceTag],
}

impl ServiceDescriptor {
    pub const fn new(
        id: &'static str,
        name: &'static str,
        description: &'static str,
        kind: ServiceKind,
    ) -> Self {
        Self {
            id,
            name,
            description,
            kind,
            critical: false,
            tags: &[],
        }
    }

    pub const fn critical(self) -> Self {
        Self {
            critical: true,
            ..self
        }
    }

    pub const fn with_tags(self, tags: &'static [ServiceTag]) -> Self {
        Self { tags, ..self }
    }

    pub fn has_tag(&self, tag: ServiceTag) -> bool {
        self.tags.iter().any(|t| t == &tag)
    }
}

#[derive(Clone, Debug)]
pub struct ServiceDescriptorOwned {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: ServiceKind,
    pub critical: bool,
    pub tags: Vec<ServiceTag>,
}

impl ServiceDescriptorOwned {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        kind: ServiceKind,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            kind,
            critical: false,
            tags: Vec::new(),
        }
    }

    pub fn with_tags(mut self, tags: impl Into<Vec<ServiceTag>>) -> Self {
        self.tags = tags.into();
        self
    }

    pub fn critical(mut self) -> Self {
        self.critical = true;
        self
    }

    pub fn has_tag(&self, tag: ServiceTag) -> bool {
        self.tags.iter().any(|t| t == &tag)
    }
}

impl From<ServiceDescriptor> for ServiceDescriptorOwned {
    fn from(value: ServiceDescriptor) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.to_string(),
            description: value.description.to_string(),
            kind: value.kind,
            critical: value.critical,
            tags: value.tags.to_vec(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceSnapshot {
    pub descriptor: ServiceDescriptorOwned,
    pub status: ServiceStatus,
    pub since: SystemTime,
    pub note: Option<String>,
}
