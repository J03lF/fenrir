use std::fmt;
use std::str::FromStr;
use std::time::SystemTime;

use uuid::Uuid;

use crate::domain::user::UserId;

pub type TicketResult<T> = Result<T, TicketError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TicketId(Uuid);

impl TicketId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for TicketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TicketId {
    type Err = TicketError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|_| TicketError::Validation("ungültige Ticket-ID".to_string()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TicketStatus {
    Open,
    InProgress,
    Resolved,
    Closed,
}

impl TicketStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TicketStatus::Open => "open",
            TicketStatus::InProgress => "in_progress",
            TicketStatus::Resolved => "resolved",
            TicketStatus::Closed => "closed",
        }
    }
}

impl fmt::Display for TicketStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TicketStatus {
    type Err = TicketError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "open" => Ok(TicketStatus::Open),
            "in_progress" | "in-progress" => Ok(TicketStatus::InProgress),
            "resolved" => Ok(TicketStatus::Resolved),
            "closed" => Ok(TicketStatus::Closed),
            other => Err(TicketError::Validation(format!(
                "unbekannter Ticket-Status: {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TicketPriority {
    Low,
    Medium,
    High,
    Critical,
}

impl TicketPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            TicketPriority::Low => "low",
            TicketPriority::Medium => "medium",
            TicketPriority::High => "high",
            TicketPriority::Critical => "critical",
        }
    }
}

impl fmt::Display for TicketPriority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TicketPriority {
    type Err = TicketError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Ok(TicketPriority::Low),
            "medium" | "normal" => Ok(TicketPriority::Medium),
            "high" => Ok(TicketPriority::High),
            "critical" | "urgent" => Ok(TicketPriority::Critical),
            other => Err(TicketError::Validation(format!(
                "unbekannte Priorität: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub id: TicketId,
    pub title: String,
    pub description: String,
    pub status: TicketStatus,
    pub priority: TicketPriority,
    pub reporter_id: UserId,
    pub assignee_id: Option<UserId>,
    pub tags: Vec<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl Ticket {
    pub fn new(
        id: TicketId,
        title: impl Into<String>,
        description: impl Into<String>,
        priority: TicketPriority,
        reporter_id: UserId,
        assignee_id: Option<UserId>,
        tags: Vec<String>,
    ) -> TicketResult<Self> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(TicketError::Validation(
                "Titel darf nicht leer sein".to_string(),
            ));
        }
        let description = description.into();
        if description.trim().is_empty() {
            return Err(TicketError::Validation(
                "Beschreibung darf nicht leer sein".to_string(),
            ));
        }
        let created_at = SystemTime::now();
        Ok(Self {
            id,
            title,
            description,
            status: TicketStatus::Open,
            priority,
            reporter_id,
            assignee_id,
            tags: sanitize_tags(tags),
            created_at,
            updated_at: created_at,
        })
    }

    pub fn set_status(&mut self, status: TicketStatus) {
        self.status = status;
        self.touch();
    }

    pub fn set_priority(&mut self, priority: TicketPriority) {
        self.priority = priority;
        self.touch();
    }

    pub fn set_assignee(&mut self, assignee_id: Option<UserId>) {
        self.assignee_id = assignee_id;
        self.touch();
    }

    pub fn update_description(&mut self, description: impl Into<String>) -> TicketResult<()> {
        let description = description.into();
        if description.trim().is_empty() {
            return Err(TicketError::Validation(
                "Beschreibung darf nicht leer sein".to_string(),
            ));
        }
        self.description = description;
        self.touch();
        Ok(())
    }

    pub fn update_title(&mut self, title: impl Into<String>) -> TicketResult<()> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(TicketError::Validation(
                "Titel darf nicht leer sein".to_string(),
            ));
        }
        self.title = title;
        self.touch();
        Ok(())
    }

    pub fn set_tags(&mut self, tags: Vec<String>) {
        self.tags = sanitize_tags(tags);
        self.touch();
    }

    fn touch(&mut self) {
        self.updated_at = SystemTime::now();
    }
}

fn sanitize_tags(mut tags: Vec<String>) -> Vec<String> {
    tags.retain(|tag| !tag.trim().is_empty());
    tags.iter_mut()
        .for_each(|tag| *tag = tag.to_ascii_lowercase());
    tags.sort();
    tags.dedup();
    tags
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TicketFilter {
    pub status: Option<Vec<TicketStatus>>,
    pub reporter: Option<UserId>,
    pub assignee: Option<UserId>,
    pub tags: Vec<String>,
    pub search: Option<String>,
}

pub trait TicketRepository: Send + Sync {
    fn insert(&self, ticket: Ticket) -> TicketResult<Ticket>;
    fn update(&self, ticket: Ticket) -> TicketResult<Ticket>;
    fn find_by_id(&self, id: &TicketId) -> TicketResult<Option<Ticket>>;
    fn list(&self, filter: &TicketFilter) -> TicketResult<Vec<Ticket>>;
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum TicketError {
    #[error("Ticket nicht gefunden")]
    NotFound,
    #[error("Validierungsfehler: {0}")]
    Validation(String),
    #[error("Ticket-Konflikt: {0}")]
    Conflict(String),
    #[error("Speicherfehler: {0}")]
    Storage(String),
}

impl TicketError {
    pub fn storage<E: fmt::Display>(err: E) -> Self {
        TicketError::Storage(err.to_string())
    }
}
