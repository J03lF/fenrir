use std::collections::HashMap;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand_core::{OsRng, RngCore};
use time::{Duration as TimeDuration, OffsetDateTime};

use crate::config::SessionSection;
use crate::security::auth::Role;

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub role: Role,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub last_activity: OffsetDateTime,
}

struct SessionEntry {
    session: Session,
}

pub struct SessionStore {
    lifetime: TimeDuration,
    idle_timeout: TimeDuration,
    cleanup_interval: Duration,
    sessions: RwLock<HashMap<String, SessionEntry>>,
    next_cleanup: Mutex<Instant>,
}

impl SessionStore {
    pub fn new(cfg: &SessionSection) -> Self {
        let lifetime = TimeDuration::seconds(cfg.lifetime_seconds as i64);
        let idle_timeout = TimeDuration::seconds(cfg.idle_timeout_seconds as i64);
        let cleanup_interval = Duration::from_secs(cfg.cleanup_interval_seconds);
        Self {
            lifetime,
            idle_timeout,
            cleanup_interval,
            sessions: RwLock::new(HashMap::new()),
            next_cleanup: Mutex::new(Instant::now() + cleanup_interval),
        }
    }

    pub fn create_session(&self, user_id: &str, role: Role) -> Result<Session, SessionError> {
        let mut token_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut token_bytes);
        let token = URL_SAFE_NO_PAD.encode(token_bytes);
        let now = OffsetDateTime::now_utc();
        let expires_at = now + self.lifetime;
        let session = Session {
            id: token.clone(),
            user_id: user_id.to_string(),
            role,
            created_at: now,
            expires_at,
            last_activity: now,
        };
        let mut guard = self.sessions.write()?;
        guard.insert(
            token.clone(),
            SessionEntry {
                session: session.clone(),
            },
        );
        Ok(session)
    }

    pub fn validate(&self, token: &str) -> Result<Session, SessionError> {
        let now = OffsetDateTime::now_utc();
        let now_instant = Instant::now();
        self.cleanup_if_needed(now_instant, now)?;

        let mut guard = self.sessions.write()?;
        match guard.get_mut(token) {
            Some(entry) => {
                if entry.session.expires_at <= now {
                    guard.remove(token);
                    return Err(SessionError::Expired);
                }
                let idle_elapsed = now - entry.session.last_activity;
                if idle_elapsed > self.idle_timeout {
                    guard.remove(token);
                    return Err(SessionError::IdleTimeout);
                }
                entry.session.last_activity = now;
                Ok(entry.session.clone())
            }
            None => Err(SessionError::NotFound),
        }
    }

    pub fn revoke(&self, token: &str) -> Result<(), SessionError> {
        let mut guard = self.sessions.write()?;
        match guard.remove(token) {
            Some(_) => Ok(()),
            None => Err(SessionError::NotFound),
        }
    }

    pub fn active_sessions(&self) -> Result<usize, SessionError> {
        Ok(self.sessions.read()?.len())
    }

    fn cleanup_if_needed(
        &self,
        now_instant: Instant,
        now: OffsetDateTime,
    ) -> Result<(), SessionError> {
        let mut schedule = self.next_cleanup.lock()?;
        if now_instant < *schedule {
            return Ok(());
        }
        *schedule = now_instant + self.cleanup_interval;
        drop(schedule);

        let mut guard = self.sessions.write()?;
        guard.retain(|_, entry| {
            if entry.session.expires_at <= now {
                return false;
            }
            let idle_elapsed = now - entry.session.last_activity;
            idle_elapsed <= self.idle_timeout
        });
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session not found")]
    NotFound,
    #[error("session expired")]
    Expired,
    #[error("session idle timeout exceeded")]
    IdleTimeout,
    #[error("session store unavailable")]
    Store,
}

impl<T> From<std::sync::PoisonError<T>> for SessionError {
    fn from(_: std::sync::PoisonError<T>) -> Self {
        SessionError::Store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::auth::Role;
    use std::thread;

    fn config() -> SessionSection {
        SessionSection {
            lifetime_seconds: 2,
            idle_timeout_seconds: 1,
            cleanup_interval_seconds: 1,
        }
    }

    #[test]
    fn session_issue_validate_and_revoke() {
        let store = SessionStore::new(&config());
        let session = store
            .create_session("test-user", Role::Viewer)
            .expect("session should be created");
        let validated = store.validate(&session.id).expect("session must validate");
        assert_eq!(validated.user_id, "test-user");
        assert_eq!(validated.role, Role::Viewer);

        store
            .revoke(&session.id)
            .expect("revoking valid session should succeed");
        assert!(matches!(
            store.validate(&session.id),
            Err(SessionError::NotFound | SessionError::Expired | SessionError::IdleTimeout)
        ));
    }

    #[test]
    fn session_idle_timeout_expires() {
        let store = SessionStore::new(&config());
        let session = store
            .create_session("idle-user", Role::Operator)
            .expect("session should be created");
        thread::sleep(Duration::from_secs(1));
        assert!(matches!(
            store.validate(&session.id),
            Err(SessionError::IdleTimeout | SessionError::NotFound)
        ));
    }
}
