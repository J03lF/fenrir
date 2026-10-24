use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::RwLock;
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

/// One issued token. `claims` never change after `issue`; `last_activity`
/// is the only mutable part and is an atomic, so the hot path (`validate`,
/// once per DB statement and gateway call) needs only the read lock.
struct ServiceTokenEntry {
    claims: DelegatedTokenClaims,
    /// Unix time of the last successful validation, in nanoseconds.
    last_activity_ns: AtomicI64,
}

impl ServiceTokenEntry {
    fn new(claims: DelegatedTokenClaims, now: OffsetDateTime) -> Self {
        Self {
            claims,
            last_activity_ns: AtomicI64::new(unix_nanos(now)),
        }
    }

    fn last_activity(&self) -> OffsetDateTime {
        let nanos = self.last_activity_ns.load(Ordering::Acquire);
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(nanos))
            .unwrap_or(OffsetDateTime::UNIX_EPOCH)
    }

    /// Never moves backwards: two validations racing with slightly different
    /// clocks keep the later one.
    fn touch(&self, now: OffsetDateTime) {
        self.last_activity_ns
            .fetch_max(unix_nanos(now), Ordering::AcqRel);
    }
}

/// Unix nanoseconds of `time`, saturated to `i64` (good until the year 2262).
fn unix_nanos(time: OffsetDateTime) -> i64 {
    i64::try_from(time.unix_timestamp_nanos()).unwrap_or(i64::MAX)
}

/// How a token stands at one point in time.
enum TokenVerdict {
    Valid,
    Expired,
    Idle,
}

pub struct ServiceTokenStore {
    lifetime: TimeDuration,
    idle_timeout: TimeDuration,
    refresh_grace: TimeDuration,
    cleanup_interval: Duration,
    tokens: RwLock<HashMap<String, ServiceTokenEntry>>,
    /// Reference point for `next_cleanup_ns`.
    epoch: Instant,
    /// When the next cleanup is due, in nanoseconds since `epoch`. The hot
    /// path only loads it; one caller per interval wins the swap and sweeps.
    next_cleanup_ns: AtomicU64,
}

impl ServiceTokenStore {
    pub fn new(cfg: &ServiceTokenSection) -> Self {
        let lifetime = TimeDuration::seconds(cfg.lifetime_seconds as i64);
        let idle_timeout = TimeDuration::seconds(cfg.idle_timeout_seconds as i64);
        let refresh_grace = TimeDuration::seconds(cfg.refresh_grace_seconds as i64);
        let cleanup_interval = Duration::from_secs(cfg.cleanup_interval_seconds);
        let epoch = Instant::now();
        Self {
            lifetime,
            idle_timeout,
            refresh_grace,
            cleanup_interval,
            tokens: RwLock::new(HashMap::new()),
            epoch,
            next_cleanup_ns: AtomicU64::new(duration_nanos(cleanup_interval)),
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
        let entry = ServiceTokenEntry::new(claims.clone(), now);
        let mut guard = self
            .tokens
            .write()
            .map_err(|_| ServiceTokenError::Store("service token store lock poisoned".into()))?;
        guard.insert(token.clone(), entry);
        Ok(DelegatedToken { token, claims })
    }

    /// Strict check: expired and idle tokens are rejected (and removed).
    ///
    /// Hot path under the *read* lock: expiry and idle time are checked
    /// against `now`, `last_activity` is bumped atomically. Only a token that
    /// turns out expired or idle takes the write lock, to remove it.
    /// Revocation stays exact: `revoke` removes under the write lock, so any
    /// validation that starts after `revoke` returned finds nothing.
    pub fn validate(&self, token: &str) -> Result<DelegatedTokenClaims, ServiceTokenError> {
        self.validate_with(token, self.lifetime_limits())
    }

    /// Like [`validate`] but tolerates tokens that expired within the
    /// configured `refresh_grace` window.  Only intended for the
    /// token-exchange endpoint (`/modules/runtime/tokens`) so that modules
    /// can recover after system sleep without a full restart.
    pub fn validate_for_refresh(
        &self,
        token: &str,
    ) -> Result<DelegatedTokenClaims, ServiceTokenError> {
        self.validate_with(token, self.refresh_limits())
    }

    pub fn revoke(&self, token: &str) -> Result<(), ServiceTokenError> {
        let mut guard = self
            .tokens
            .write()
            .map_err(|_| ServiceTokenError::Store("service token store lock poisoned".into()))?;
        guard.remove(token);
        Ok(())
    }

    fn lifetime_limits(&self) -> Limits {
        Limits {
            expiry_grace: TimeDuration::ZERO,
            idle: self.idle_timeout,
        }
    }

    fn refresh_limits(&self) -> Limits {
        Limits {
            expiry_grace: self.refresh_grace,
            idle: self.idle_timeout + self.refresh_grace,
        }
    }

    fn verdict(entry: &ServiceTokenEntry, now: OffsetDateTime, limits: Limits) -> TokenVerdict {
        if entry.claims.expires_at + limits.expiry_grace <= now {
            return TokenVerdict::Expired;
        }
        if now - entry.last_activity() > limits.idle {
            return TokenVerdict::Idle;
        }
        TokenVerdict::Valid
    }

    fn validate_with(
        &self,
        token: &str,
        limits: Limits,
    ) -> Result<DelegatedTokenClaims, ServiceTokenError> {
        let now = OffsetDateTime::now_utc();
        self.cleanup_if_needed(now)?;
        {
            let guard = self.tokens.read().map_err(|_| {
                ServiceTokenError::Store("service token store lock poisoned".into())
            })?;
            let Some(entry) = guard.get(token) else {
                return Err(ServiceTokenError::NotFound);
            };
            if matches!(Self::verdict(entry, now, limits), TokenVerdict::Valid) {
                entry.touch(now);
                return Ok(entry.claims.clone());
            }
        }
        // Expired or idle: remove it under the write lock. Re-check first,
        // the entry may have been revoked, replaced or touched meanwhile.
        let mut guard = self
            .tokens
            .write()
            .map_err(|_| ServiceTokenError::Store("service token store lock poisoned".into()))?;
        let Some(entry) = guard.get(token) else {
            return Err(ServiceTokenError::NotFound);
        };
        match Self::verdict(entry, now, limits) {
            TokenVerdict::Valid => {
                entry.touch(now);
                Ok(entry.claims.clone())
            }
            TokenVerdict::Expired => {
                guard.remove(token);
                Err(ServiceTokenError::Expired)
            }
            TokenVerdict::Idle => {
                guard.remove(token);
                Err(ServiceTokenError::IdleTimeout)
            }
        }
    }

    /// Sweeps tokens past their grace window, at most once per
    /// `cleanup_interval`. Every other caller pays one atomic load.
    fn cleanup_if_needed(&self, now: OffsetDateTime) -> Result<(), ServiceTokenError> {
        let elapsed = duration_nanos(self.epoch.elapsed());
        let due = self.next_cleanup_ns.load(Ordering::Acquire);
        if elapsed < due {
            return Ok(());
        }
        let next = elapsed.saturating_add(duration_nanos(self.cleanup_interval));
        if self
            .next_cleanup_ns
            .compare_exchange(due, next, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // Another caller won this interval's sweep.
            return Ok(());
        }

        let grace = self.refresh_grace;
        let idle_limit = self.idle_timeout;
        let mut guard = self
            .tokens
            .write()
            .map_err(|_| ServiceTokenError::Store("service token store lock poisoned".into()))?;
        guard.retain(|_, entry| {
            if entry.claims.expires_at + grace <= now {
                return false;
            }
            let idle_elapsed = now - entry.last_activity();
            idle_elapsed <= idle_limit + grace
        });
        Ok(())
    }

    fn generate_token() -> String {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        URL_SAFE_NO_PAD.encode(bytes)
    }
}

/// Expiry grace and idle limit of one kind of validation.
#[derive(Clone, Copy)]
struct Limits {
    expiry_grace: TimeDuration,
    idle: TimeDuration,
}

fn duration_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
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
#[path = "../../tests/unit/security/service_tokens_tests.rs"]
mod tests;
