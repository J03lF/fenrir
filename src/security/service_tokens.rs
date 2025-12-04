use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand_core::{OsRng, RngCore};
use time::{Duration as TimeDuration, OffsetDateTime};
use uuid::Uuid;

use crate::config::ServiceTokenSection;
use crate::security::auth::Role;
use crate::security::service::{ServiceRole, ServiceScope};

#[derive(Debug, Clone)]
pub enum DelegatedActor {
    User {
        user_id: String,
        role: Role,
    },
    Service {
        service_id: String,
        role: ServiceRole,
    },
}

impl DelegatedActor {
    pub fn kind(&self) -> &'static str {
        match self {
            DelegatedActor::User { .. } => "user",
            DelegatedActor::Service { .. } => "service",
        }
    }

    pub fn identifier(&self) -> &str {
        match self {
            DelegatedActor::User { user_id, .. } => user_id,
            DelegatedActor::Service { service_id, .. } => service_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DelegatedTokenRequest {
    actor: DelegatedActor,
    tenant_id: String,
    scopes: Vec<ServiceScope>,
}

impl DelegatedTokenRequest {
    pub fn new(actor: DelegatedActor, tenant_id: impl Into<String>) -> Self {
        Self {
            actor,
            tenant_id: tenant_id.into(),
            scopes: Vec::new(),
        }
    }

    pub fn with_scopes(mut self, scopes: Vec<ServiceScope>) -> Self {
        self.scopes = scopes;
        self
    }

    pub fn actor(&self) -> &DelegatedActor {
        &self.actor
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn scopes(&self) -> &[ServiceScope] {
        &self.scopes
    }
}

#[derive(Debug, Clone)]
pub struct DelegatedToken {
    pub token: String,
    pub claims: DelegatedTokenClaims,
}

#[derive(Debug, Clone)]
pub struct DelegatedTokenClaims {
    pub token_id: String,
    pub actor: DelegatedActor,
    pub tenant_id: String,
    pub scopes: Vec<ServiceScope>,
    pub issued_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

struct ServiceTokenEntry {
    claims: DelegatedTokenClaims,
    last_activity: OffsetDateTime,
}

pub struct ServiceTokenStore {
    lifetime: TimeDuration,
    idle_timeout: TimeDuration,
    cleanup_interval: Duration,
    tokens: RwLock<HashMap<String, ServiceTokenEntry>>,
    next_cleanup: Mutex<Instant>,
}

impl ServiceTokenStore {
    pub fn new(cfg: &ServiceTokenSection) -> Self {
        let lifetime = TimeDuration::seconds(cfg.lifetime_seconds as i64);
        let idle_timeout = TimeDuration::seconds(cfg.idle_timeout_seconds as i64);
        let cleanup_interval = Duration::from_secs(cfg.cleanup_interval_seconds);
        Self {
            lifetime,
            idle_timeout,
            cleanup_interval,
            tokens: RwLock::new(HashMap::new()),
            next_cleanup: Mutex::new(Instant::now() + cleanup_interval),
        }
    }

    pub fn issue(
        &self,
        request: DelegatedTokenRequest,
    ) -> Result<DelegatedToken, ServiceTokenError> {
        let token = Self::generate_token();
        let now = OffsetDateTime::now_utc();
        let expires_at = now + self.lifetime;
        let claims = DelegatedTokenClaims {
            token_id: Uuid::new_v4().to_string(),
            actor: request.actor,
            tenant_id: request.tenant_id,
            scopes: request.scopes,
            issued_at: now,
            expires_at,
        };
        let entry = ServiceTokenEntry {
            claims: claims.clone(),
            last_activity: now,
        };
        let mut guard = self
            .tokens
            .write()
            .map_err(|_| ServiceTokenError::Store("service token store lock poisoned".into()))?;
        guard.insert(token.clone(), entry);
        Ok(DelegatedToken { token, claims })
    }

    pub fn validate(&self, token: &str) -> Result<DelegatedTokenClaims, ServiceTokenError> {
        let now = OffsetDateTime::now_utc();
        let instant_now = Instant::now();
        self.cleanup_if_needed(instant_now, now)?;
        let mut guard = self
            .tokens
            .write()
            .map_err(|_| ServiceTokenError::Store("service token store lock poisoned".into()))?;
        match guard.get_mut(token) {
            Some(entry) => {
                if entry.claims.expires_at <= now {
                    guard.remove(token);
                    return Err(ServiceTokenError::Expired);
                }
                let idle_elapsed = now - entry.last_activity;
                if idle_elapsed > self.idle_timeout {
                    guard.remove(token);
                    return Err(ServiceTokenError::IdleTimeout);
                }
                entry.last_activity = now;
                Ok(entry.claims.clone())
            }
            None => Err(ServiceTokenError::NotFound),
        }
    }

    pub fn revoke(&self, token: &str) -> Result<(), ServiceTokenError> {
        let mut guard = self
            .tokens
            .write()
            .map_err(|_| ServiceTokenError::Store("service token store lock poisoned".into()))?;
        guard.remove(token);
        Ok(())
    }

    fn cleanup_if_needed(
        &self,
        instant_now: Instant,
        now: OffsetDateTime,
    ) -> Result<(), ServiceTokenError> {
        let mut schedule = self
            .next_cleanup
            .lock()
            .map_err(|_| ServiceTokenError::Store("service token cleanup lock poisoned".into()))?;
        if instant_now < *schedule {
            return Ok(());
        }
        *schedule = instant_now + self.cleanup_interval;
        drop(schedule);

        let mut guard = self
            .tokens
            .write()
            .map_err(|_| ServiceTokenError::Store("service token store lock poisoned".into()))?;
        guard.retain(|_, entry| {
            if entry.claims.expires_at <= now {
                return false;
            }
            let idle_elapsed = now - entry.last_activity;
            idle_elapsed <= self.idle_timeout
        });
        Ok(())
    }

    fn generate_token() -> String {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    }
}

#[derive(Debug)]
pub enum ServiceTokenError {
    NotFound,
    Expired,
    IdleTimeout,
    Store(String),
}

impl fmt::Display for ServiceTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceTokenError::NotFound => f.write_str("service token not found"),
            ServiceTokenError::Expired => f.write_str("service token expired"),
            ServiceTokenError::IdleTimeout => f.write_str("service token idle timeout"),
            ServiceTokenError::Store(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ServiceTokenError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ServiceTokenSection {
        ServiceTokenSection {
            lifetime_seconds: 2,
            idle_timeout_seconds: 1,
            cleanup_interval_seconds: 1,
        }
    }

    #[test]
    fn issue_validate_and_revoke() {
        let store = ServiceTokenStore::new(&config());
        let request = DelegatedTokenRequest::new(
            DelegatedActor::Service {
                service_id: "module:test".to_string(),
                role: ServiceRole::Write,
            },
            "tenant-1",
        );
        let issued = store.issue(request).expect("token issued");
        let validated = store.validate(&issued.token).expect("token validates");
        assert_eq!(validated.tenant_id, "tenant-1");
        store.revoke(&issued.token).expect("revoke ok");
        assert!(matches!(
            store.validate(&issued.token),
            Err(ServiceTokenError::NotFound)
        ));
    }

    #[test]
    fn idle_timeout_expires_tokens() {
        let store = ServiceTokenStore::new(&config());
        let issued = store
            .issue(DelegatedTokenRequest::new(
                DelegatedActor::Service {
                    service_id: "module:test".to_string(),
                    role: ServiceRole::Read,
                },
                "tenant-1",
            ))
            .expect("token issued");
        std::thread::sleep(Duration::from_millis(1100));
        assert!(matches!(
            store.validate(&issued.token),
            Err(ServiceTokenError::IdleTimeout | ServiceTokenError::NotFound)
        ));
    }
}
