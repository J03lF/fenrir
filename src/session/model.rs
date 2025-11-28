use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::security::auth::Role;

use super::id::SessionId;

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
