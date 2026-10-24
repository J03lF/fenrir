//! Who runs a statement and how (step 5.6): the key of the connection pool
//! it may use, whether it must run read-only, and what it may leave behind
//! in the session.

/// The connection pool a caller may use. Connections are never shared
/// between keys: one key per module service (and tenant) of the DB
/// connector, one for the host's own tools.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DbPoolKey(String);

impl DbPoolKey {
    /// The host's own callers: CLI and SSH shell, migrations, schema tools.
    pub fn host() -> Self {
        Self("host".to_string())
    }

    /// A module service of the DB connector, from its token claims.
    pub fn service(service_id: &str, tenant_id: &str) -> Self {
        Self(format!("service:{service_id}@{tenant_id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Read-only statements run in a `READ ONLY` transaction, whatever they say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbAccess {
    ReadOnly,
    ReadWrite,
}

/// What a statement may leave behind in its database session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbSessionEffect {
    /// Nothing once it completed (plain DML, reads).
    None,
    /// Session state may survive (`SET`, transactions, temp tables, …):
    /// reset the connection before it is reused.
    Resettable,
    /// Prepared statements or the whole session were touched: close the
    /// connection instead of reusing it.
    Unrecoverable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbCallContext {
    pub pool: DbPoolKey,
    pub access: DbAccess,
    pub session: DbSessionEffect,
}

impl DbCallContext {
    /// The host's own tools: read-write, and every call may leave state.
    pub fn host() -> Self {
        Self {
            pool: DbPoolKey::host(),
            access: DbAccess::ReadWrite,
            session: DbSessionEffect::Resettable,
        }
    }
}

/// Counters of one pool, without anything secret (no URI, no credentials).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbPoolKeyStatus {
    pub key: String,
    pub open: u64,
    pub idle: u64,
    pub in_use: u64,
    pub waiting: u64,
    pub created: u64,
    pub reused: u64,
    pub closed: u64,
    pub resets: u64,
    pub wait_timeouts: u64,
    pub connect_failures: u64,
    pub statement_cache_hits: u64,
    pub statement_cache_misses: u64,
    pub statement_cache_invalidations: u64,
    pub retries: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbPoolStatus {
    pub max_per_pool: u64,
    pub max_total: u64,
    pub open_total: u64,
    pub in_use_total: u64,
    pub pools: Vec<DbPoolKeyStatus>,
}
