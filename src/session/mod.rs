use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::sync::RwLock;
use std::time::{Duration, SystemTime};

use uuid::Uuid;

use crate::security::auth::Role;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SessionId {
    type Err = SessionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s)
            .map(SessionId)
            .map_err(|_| SessionError::Invalid("ungültige Session-ID".to_string()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub user_id: String,
    pub role: Role,
    pub created_at: SystemTime,
    pub expires_at: SystemTime,
    pub metadata: HashMap<String, String>,
}

impl Session {
    pub fn is_expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at
    }
}

#[derive(Clone, Debug)]
pub struct SessionBuilder {
    user_id: String,
    role: Role,
    ttl: Duration,
    metadata: HashMap<String, String>,
}

impl SessionBuilder {
    pub fn new(user_id: impl Into<String>, role: Role, ttl: Duration) -> Self {
        Self {
            user_id: user_id.into(),
            role,
            ttl,
            metadata: HashMap::new(),
        }
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> Session {
        let now = SystemTime::now();
        Session {
            id: SessionId::new(),
            user_id: self.user_id,
            role: self.role,
            created_at: now,
            expires_at: now + self.ttl,
            metadata: self.metadata,
        }
    }
}

pub trait SessionStore: Send + Sync {
    fn create(&self, session: Session) -> Result<Session, SessionError>;
    fn get(&self, id: &SessionId) -> Result<Option<Session>, SessionError>;
    fn invalidate(&self, id: &SessionId) -> Result<(), SessionError>;
    fn cleanup_expired(&self) -> Result<usize, SessionError>;
}

pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<SessionId, Session>>, // TTL enforced on access/cleanup
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStore for InMemorySessionStore {
    fn create(&self, session: Session) -> Result<Session, SessionError> {
        let mut guard = self
            .sessions
            .write()
            .map_err(|_| SessionError::Storage("Session-Store gesperrt".to_string()))?;
        guard.insert(session.id, session.clone());
        Ok(session)
    }

    fn get(&self, id: &SessionId) -> Result<Option<Session>, SessionError> {
        let mut guard = self
            .sessions
            .write()
            .map_err(|_| SessionError::Storage("Session-Store gesperrt".to_string()))?;
        if let Some(session) = guard.get(id) {
            if session.is_expired(SystemTime::now()) {
                guard.remove(id);
                return Ok(None);
            }
            return Ok(Some(session.clone()));
        }
        Ok(None)
    }

    fn invalidate(&self, id: &SessionId) -> Result<(), SessionError> {
        let mut guard = self
            .sessions
            .write()
            .map_err(|_| SessionError::Storage("Session-Store gesperrt".to_string()))?;
        guard.remove(id);
        Ok(())
    }

    fn cleanup_expired(&self) -> Result<usize, SessionError> {
        let mut guard = self
            .sessions
            .write()
            .map_err(|_| SessionError::Storage("Session-Store gesperrt".to_string()))?;
        let now = SystemTime::now();
        let before = guard.len();
        guard.retain(|_, session| !session.is_expired(now));
        Ok(before.saturating_sub(guard.len()))
    }
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum SessionError {
    #[error("ungültige Session: {0}")]
    Invalid(String),
    #[error("Storage-Fehler: {0}")]
    Storage(String),
}

impl SessionError {
    #[allow(dead_code)]
    pub fn storage<E: fmt::Display>(err: E) -> Self {
        SessionError::Storage(err.to_string())
    }
}
