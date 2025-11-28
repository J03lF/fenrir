use std::collections::HashMap;
use std::sync::RwLock;
use std::time::SystemTime;

use crate::utils::messages::session::store as session_store_messages;

use super::{Session, SessionError, SessionId};

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
            .map_err(|_| SessionError::Storage(session_store_messages::LOCKED.to_string()))?;
        guard.insert(session.id, session.clone());
        Ok(session)
    }

    fn get(&self, id: &SessionId) -> Result<Option<Session>, SessionError> {
        let mut guard = self
            .sessions
            .write()
            .map_err(|_| SessionError::Storage(session_store_messages::LOCKED.to_string()))?;
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
            .map_err(|_| SessionError::Storage(session_store_messages::LOCKED.to_string()))?;
        guard.remove(id);
        Ok(())
    }

    fn cleanup_expired(&self) -> Result<usize, SessionError> {
        let mut guard = self
            .sessions
            .write()
            .map_err(|_| SessionError::Storage(session_store_messages::LOCKED.to_string()))?;
        let now = SystemTime::now();
        let before = guard.len();
        guard.retain(|_, session| !session.is_expired(now));
        Ok(before.saturating_sub(guard.len()))
    }
}
