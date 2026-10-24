//! Connection pool of the Postgres adapter (step 5.6).
//!
//! - One pool per [`DbPoolKey`] (a module service of the DB connector, or the
//!   host itself); a connection never moves between keys. The adapter has
//!   one URI, i.e. one database and one role, so key × adapter is the
//!   (module, database, role) triple.
//! - Limits: `max_per_pool` connections in use per key, `max_total` in use
//!   over all keys, and at most `max_total` open (idle ones of other keys are
//!   closed first). A checkout waits up to `wait_timeout`, then fails.
//! - Connections are checked when taken: closed by the server (e.g. after a
//!   Postgres restart), older than `max_lifetime` or idle for longer than
//!   `idle_timeout` → closed and replaced; idle for longer than
//!   `health_check_after` → pinged first.
//! - Every connection keeps its own prepared-statement cache
//!   ([`StatementCache`]); [`PgPool::invalidate_statement_caches`] empties all
//!   of them (after migrations).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::{Client, Statement};

use crate::domain::db::{DbError, DbPoolKey, DbPoolKeyStatus, DbPoolStatus, DbResult};

/// Opens one new, authenticated connection for the given pool.
pub(crate) type ConnectFn =
    Arc<dyn Fn(&DbPoolKey) -> Pin<Box<dyn Future<Output = DbResult<Client>> + Send>> + Send + Sync>;

#[derive(Debug, Clone)]
pub(crate) struct PoolSettings {
    pub max_per_pool: usize,
    pub max_total: usize,
    pub wait_timeout: Duration,
    pub connect_timeout: Duration,
    pub reset_timeout: Duration,
    pub max_lifetime: Duration,
    pub idle_timeout: Duration,
    pub health_check_after: Duration,
    pub statement_cache: usize,
    pub sweep_interval: Duration,
    /// `statement_timeout` of module connections (not the host's own pool).
    pub module_statement_timeout: Option<Duration>,
}

impl Default for PoolSettings {
    fn default() -> Self {
        Self {
            // Embedded Postgres allows 100 connections; 5 modules × 8 plus
            // the host stay well below, with room for backups and psql.
            max_per_pool: 8,
            max_total: 48,
            wait_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(5),
            reset_timeout: Duration::from_secs(2),
            max_lifetime: Duration::from_secs(30 * 60),
            idle_timeout: Duration::from_secs(5 * 60),
            health_check_after: Duration::from_secs(30),
            statement_cache: 256,
            sweep_interval: Duration::from_secs(30),
            module_statement_timeout: Some(Duration::from_secs(30)),
        }
    }
}

/// Prepared statements of one connection, least recently used evicted.
pub(crate) struct StatementCache {
    capacity: usize,
    tick: u64,
    entries: HashMap<String, (Statement, u64)>,
}

impl StatementCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            tick: 0,
            entries: HashMap::new(),
        }
    }

    pub(crate) fn get(&mut self, sql: &str) -> Option<Statement> {
        self.tick = self.tick.wrapping_add(1);
        let tick = self.tick;
        self.entries.get_mut(sql).map(|(statement, used)| {
            *used = tick;
            statement.clone()
        })
    }

    pub(crate) fn insert(&mut self, sql: &str, statement: Statement) {
        if self.entries.len() >= self.capacity && !self.entries.contains_key(sql) {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone())
            {
                // Dropping the `Statement` closes it on the server.
                self.entries.remove(&oldest);
            }
        }
        self.tick = self.tick.wrapping_add(1);
        self.entries.insert(sql.to_string(), (statement, self.tick));
    }

    pub(crate) fn remove(&mut self, sql: &str) {
        self.entries.remove(sql);
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// One open connection.
pub(crate) struct PgConn {
    pub(crate) id: u64,
    pub(crate) client: Client,
    pub(crate) statements: StatementCache,
    created: Instant,
    last_used: Instant,
    generation: u64,
}

#[derive(Default)]
struct KeyStats {
    created: AtomicU64,
    reused: AtomicU64,
    closed: AtomicU64,
    resets: AtomicU64,
    wait_timeouts: AtomicU64,
    connect_failures: AtomicU64,
    waiting: AtomicU64,
    in_use: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    cache_invalidations: AtomicU64,
    retries: AtomicU64,
}

struct KeyPool {
    key: DbPoolKey,
    permits: Arc<Semaphore>,
    idle: Mutex<Vec<PgConn>>,
    stats: KeyStats,
}

impl KeyPool {
    fn idle_len(&self) -> usize {
        self.idle.lock().map(|idle| idle.len()).unwrap_or(0)
    }
}

pub(crate) struct PgPool {
    settings: PoolSettings,
    connect: ConnectFn,
    pools: RwLock<HashMap<DbPoolKey, Arc<KeyPool>>>,
    in_use_total: Arc<Semaphore>,
    open_total: Arc<AtomicU64>,
    next_id: AtomicU64,
    generation: AtomicU64,
    last_sweep: Mutex<Instant>,
}

impl PgPool {
    pub(crate) fn new(settings: PoolSettings, connect: ConnectFn) -> Self {
        let in_use_total = Arc::new(Semaphore::new(settings.max_total.max(1)));
        Self {
            settings,
            connect,
            pools: RwLock::new(HashMap::new()),
            in_use_total,
            open_total: Arc::new(AtomicU64::new(0)),
            next_id: AtomicU64::new(1),
            generation: AtomicU64::new(0),
            last_sweep: Mutex::new(Instant::now()),
        }
    }

    fn pool_for(&self, key: &DbPoolKey) -> Arc<KeyPool> {
        if let Ok(pools) = self.pools.read() {
            if let Some(pool) = pools.get(key) {
                return Arc::clone(pool);
            }
        }
        let mut pools = match self.pools.write() {
            Ok(pools) => pools,
            Err(poisoned) => poisoned.into_inner(),
        };
        Arc::clone(pools.entry(key.clone()).or_insert_with(|| {
            Arc::new(KeyPool {
                key: key.clone(),
                permits: Arc::new(Semaphore::new(self.settings.max_per_pool.max(1))),
                idle: Mutex::new(Vec::new()),
                stats: KeyStats::default(),
            })
        }))
    }

    /// Replaces how new connections are opened (idle ones stay). Only used
    /// while the adapter is being built.
    pub(crate) fn set_connect(&mut self, connect: ConnectFn) {
        self.connect = connect;
    }

    /// Empties every connection's statement cache before its next use.
    pub(crate) fn invalidate_statement_caches(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// A connection for `key`: an idle one when it is healthy, else a new
    /// one. Waits for a free slot up to `wait_timeout`.
    pub(crate) async fn get(&self, key: &DbPoolKey) -> DbResult<PooledConn> {
        self.sweep_if_due();
        let pool = self.pool_for(key);
        pool.stats.waiting.fetch_add(1, Ordering::AcqRel);
        let deadline = tokio::time::Instant::now() + self.settings.wait_timeout;
        let permits = async {
            let key_permit = Arc::clone(&pool.permits).acquire_owned().await;
            let total_permit = Arc::clone(&self.in_use_total).acquire_owned().await;
            (key_permit, total_permit)
        };
        let acquired = tokio::time::timeout_at(deadline, permits).await;
        pool.stats.waiting.fetch_sub(1, Ordering::AcqRel);
        let (key_permit, total_permit) = match acquired {
            Ok((Ok(key_permit), Ok(total_permit))) => (key_permit, total_permit),
            Ok(_) => return Err(DbError::connection("connection pool closed")),
            Err(_) => {
                pool.stats.wait_timeouts.fetch_add(1, Ordering::Relaxed);
                return Err(DbError::connection(format!(
                    "no database connection free within {} ms (pool '{}')",
                    self.settings.wait_timeout.as_millis(),
                    pool.key.as_str()
                )));
            }
        };
        pool.stats.in_use.fetch_add(1, Ordering::AcqRel);
        let mut guard = PooledConn {
            conn: None,
            pool: Arc::clone(&pool),
            open_total: Arc::clone(&self.open_total),
            reused: false,
            discard: false,
            _key_permit: key_permit,
            _total_permit: total_permit,
        };

        let generation = self.generation.load(Ordering::Acquire);
        while let Some(mut conn) = pool.idle.lock().ok().and_then(|mut idle| idle.pop()) {
            if conn.client.is_closed()
                || conn.created.elapsed() > self.settings.max_lifetime
                || conn.last_used.elapsed() > self.settings.idle_timeout
            {
                self.close(&pool, conn);
                continue;
            }
            if conn.last_used.elapsed() > self.settings.health_check_after {
                let ping = tokio::time::timeout(
                    self.settings.reset_timeout,
                    conn.client.batch_execute(""),
                )
                .await;
                if !matches!(ping, Ok(Ok(()))) {
                    self.close(&pool, conn);
                    continue;
                }
            }
            if conn.generation != generation {
                if conn.statements.len() > 0 {
                    pool.stats
                        .cache_invalidations
                        .fetch_add(1, Ordering::Relaxed);
                }
                conn.statements.clear();
                conn.generation = generation;
            }
            pool.stats.reused.fetch_add(1, Ordering::Relaxed);
            guard.reused = true;
            guard.conn = Some(conn);
            return Ok(guard);
        }

        self.make_room();
        let connect = (self.connect)(&pool.key);
        let client = match tokio::time::timeout(self.settings.connect_timeout, connect).await {
            Ok(Ok(client)) => client,
            Ok(Err(err)) => {
                pool.stats.connect_failures.fetch_add(1, Ordering::Relaxed);
                return Err(err);
            }
            Err(_) => {
                pool.stats.connect_failures.fetch_add(1, Ordering::Relaxed);
                return Err(DbError::connection(format!(
                    "database connection not established within {} ms",
                    self.settings.connect_timeout.as_millis()
                )));
            }
        };
        self.open_total.fetch_add(1, Ordering::AcqRel);
        pool.stats.created.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        guard.conn = Some(PgConn {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            client,
            statements: StatementCache::new(self.settings.statement_cache),
            created: now,
            last_used: now,
            generation,
        });
        Ok(guard)
    }

    fn close(&self, pool: &KeyPool, conn: PgConn) {
        drop(conn);
        self.open_total.fetch_sub(1, Ordering::AcqRel);
        pool.stats.closed.fetch_add(1, Ordering::Relaxed);
    }

    /// Before opening a connection at the global limit, close the least
    /// recently used idle connection of any key.
    fn make_room(&self) {
        let max_total = u64::try_from(self.settings.max_total).unwrap_or(u64::MAX);
        if self.open_total.load(Ordering::Acquire) < max_total {
            return;
        }
        let pools: Vec<Arc<KeyPool>> = match self.pools.read() {
            Ok(pools) => pools.values().cloned().collect(),
            Err(_) => return,
        };
        let mut oldest: Option<(Arc<KeyPool>, Instant)> = None;
        for pool in pools {
            let candidate = pool
                .idle
                .lock()
                .ok()
                .and_then(|idle| idle.iter().map(|conn| conn.last_used).min());
            if let Some(last_used) = candidate {
                if oldest.as_ref().is_none_or(|(_, best)| last_used < *best) {
                    oldest = Some((pool, last_used));
                }
            }
        }
        if let Some((pool, last_used)) = oldest {
            let conn = pool.idle.lock().ok().and_then(|mut idle| {
                let index = idle.iter().position(|conn| conn.last_used == last_used)?;
                Some(idle.swap_remove(index))
            });
            if let Some(conn) = conn {
                self.close(&pool, conn);
            }
        }
    }

    /// Closes idle connections past their lifetime or idle limit, at most
    /// once per `sweep_interval`.
    fn sweep_if_due(&self) {
        {
            let Ok(mut last) = self.last_sweep.try_lock() else {
                return;
            };
            if last.elapsed() < self.settings.sweep_interval {
                return;
            }
            *last = Instant::now();
        }
        let pools: Vec<Arc<KeyPool>> = match self.pools.read() {
            Ok(pools) => pools.values().cloned().collect(),
            Err(_) => return,
        };
        for pool in pools {
            let expired: Vec<PgConn> = match pool.idle.lock() {
                Ok(mut idle) => {
                    let (keep, expired): (Vec<_>, Vec<_>) = idle.drain(..).partition(|conn| {
                        !conn.client.is_closed()
                            && conn.created.elapsed() <= self.settings.max_lifetime
                            && conn.last_used.elapsed() <= self.settings.idle_timeout
                    });
                    *idle = keep;
                    expired
                }
                Err(_) => continue,
            };
            for conn in expired {
                self.close(&pool, conn);
            }
        }
    }

    pub(crate) fn status(&self) -> DbPoolStatus {
        let pools: Vec<Arc<KeyPool>> = match self.pools.read() {
            Ok(pools) => pools.values().cloned().collect(),
            Err(_) => Vec::new(),
        };
        let mut views: Vec<DbPoolKeyStatus> = pools
            .iter()
            .map(|pool| {
                let stats = &pool.stats;
                let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
                let idle = u64::try_from(pool.idle_len()).unwrap_or(u64::MAX);
                let in_use = load(&stats.in_use);
                DbPoolKeyStatus {
                    key: pool.key.as_str().to_string(),
                    open: idle.saturating_add(in_use),
                    idle,
                    in_use,
                    waiting: load(&stats.waiting),
                    created: load(&stats.created),
                    reused: load(&stats.reused),
                    closed: load(&stats.closed),
                    resets: load(&stats.resets),
                    wait_timeouts: load(&stats.wait_timeouts),
                    connect_failures: load(&stats.connect_failures),
                    statement_cache_hits: load(&stats.cache_hits),
                    statement_cache_misses: load(&stats.cache_misses),
                    statement_cache_invalidations: load(&stats.cache_invalidations),
                    retries: load(&stats.retries),
                }
            })
            .collect();
        views.sort_by(|a, b| a.key.cmp(&b.key));
        let in_use_total = views.iter().map(|pool| pool.in_use).sum();
        DbPoolStatus {
            max_per_pool: u64::try_from(self.settings.max_per_pool).unwrap_or(u64::MAX),
            max_total: u64::try_from(self.settings.max_total).unwrap_or(u64::MAX),
            open_total: self.open_total.load(Ordering::Acquire),
            in_use_total,
            pools: views,
        }
    }
}

/// A checked-out connection. Dropping it returns the connection to its pool
/// (unless it was marked for discard or is closed) and frees the slot.
pub(crate) struct PooledConn {
    conn: Option<PgConn>,
    pool: Arc<KeyPool>,
    open_total: Arc<AtomicU64>,
    reused: bool,
    discard: bool,
    // Dropped after `Drop::drop` put the connection back, so a waiter that
    // gets the slot finds it idle.
    _key_permit: OwnedSemaphorePermit,
    _total_permit: OwnedSemaphorePermit,
}

impl PooledConn {
    pub(crate) fn conn(&mut self) -> DbResult<&mut PgConn> {
        self.conn
            .as_mut()
            .ok_or_else(|| DbError::connection("pooled connection already released"))
    }

    pub(crate) fn reused(&self) -> bool {
        self.reused
    }

    /// Close this connection instead of returning it.
    pub(crate) fn discard(&mut self) {
        self.discard = true;
    }

    pub(crate) fn record_cache(&self, hit: bool) {
        let counter = if hit {
            &self.pool.stats.cache_hits
        } else {
            &self.pool.stats.cache_misses
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_invalidation(&self) {
        self.pool
            .stats
            .cache_invalidations
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_retry(&self) {
        self.pool.stats.retries.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_reset(&self) {
        self.pool.stats.resets.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        self.pool.stats.in_use.fetch_sub(1, Ordering::AcqRel);
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        if self.discard || conn.client.is_closed() {
            drop(conn);
            self.open_total.fetch_sub(1, Ordering::AcqRel);
            self.pool.stats.closed.fetch_add(1, Ordering::Relaxed);
            return;
        }
        conn.last_used = Instant::now();
        match self.pool.idle.lock() {
            Ok(mut idle) => idle.push(conn),
            Err(_) => {
                drop(conn);
                self.open_total.fetch_sub(1, Ordering::AcqRel);
                self.pool.stats.closed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
