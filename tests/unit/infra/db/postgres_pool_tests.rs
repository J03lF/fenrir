//! The Postgres adapter's pool against a real server (step 5.6).
//!
//! Needs a throwaway cluster: `FENRIR_PG_TEST_URI_FILE` names a 0600 file
//! with its URI; for the restart test also `FENRIR_PG_TEST_PGCTL` (pg_ctl),
//! `FENRIR_PG_TEST_DATA` (data directory) and `FENRIR_PG_TEST_RESTART=1`.
//! Without them every test here returns early (and says so on stderr).

use super::*;
use crate::domain::db::{DbAccess, DbCallContext, DbPoolKey, DbSessionEffect};
use crate::infra::db::adapters::postgres::pool::PoolSettings;
use std::collections::HashSet;
use std::time::Instant;

fn test_uri() -> Option<String> {
    let path = std::env::var("FENRIR_PG_TEST_URI_FILE").ok()?;
    let uri = std::fs::read_to_string(path).ok()?;
    Some(uri.trim().to_string())
}

macro_rules! require_pg {
    () => {
        match test_uri() {
            Some(uri) => uri,
            None => {
                eprintln!("skipped: FENRIR_PG_TEST_URI_FILE not set");
                return;
            }
        }
    };
}

fn adapter_with(uri: &str, settings: PoolSettings) -> PostgresAdapter {
    PostgresAdapter::with_settings(uri, settings, Some(Duration::from_secs(10))).unwrap()
}

fn adapter(uri: &str) -> PostgresAdapter {
    adapter_with(uri, PoolSettings::default())
}

fn module(name: &str, access: DbAccess, session: DbSessionEffect) -> DbCallContext {
    DbCallContext {
        pool: DbPoolKey::service(name, "tenant"),
        access,
        session,
    }
}

fn reader(name: &str) -> DbCallContext {
    module(name, DbAccess::ReadOnly, DbSessionEffect::None)
}

fn writer(name: &str) -> DbCallContext {
    module(name, DbAccess::ReadWrite, DbSessionEffect::None)
}

fn first_value(results: &[DbExecutionResult]) -> String {
    match results.first() {
        Some(DbExecutionResult::ResultSet(set)) => set.rows[0][0].clone(),
        other => panic!("no result set: {other:?}"),
    }
}

async fn value(adapter: &PostgresAdapter, ctx: &DbCallContext, sql: &str) -> String {
    let results = adapter.prepared_query_in(ctx, sql, &[]).await.unwrap();
    first_value(&results)
}

fn pool_of<'a>(status: &'a DbPoolStatus, ctx: &DbCallContext) -> &'a DbPoolKeyStatusView {
    status
        .pools
        .iter()
        .find(|pool| pool.key == ctx.pool.as_str())
        .expect("pool exists")
}

type DbPoolKeyStatusView = crate::domain::db::DbPoolKeyStatus;

/// No transaction is open on the pool's connection: `SAVEPOINT` fails
/// outside a transaction block, and `now()` moves between two statements
/// (inside one open transaction it would stand still).
async fn assert_no_open_transaction(adapter: &PostgresAdapter, ctx: &DbCallContext) {
    let savepoint = adapter
        .prepared_execute_in(ctx, "SAVEPOINT hp_probe", &[])
        .await;
    assert!(
        savepoint.is_err(),
        "SAVEPOINT worked: a transaction is open"
    );
    let first = value(adapter, ctx, "SELECT now()::text").await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = value(adapter, ctx, "SELECT now()::text").await;
    assert_ne!(first, second, "now() stood still: a transaction is open");
}

/// A table name unique to one test run.
fn table(name: &str) -> String {
    format!(
        "hp_{name}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            % 1_000_000_000
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_and_statements_are_reused() {
    let uri = require_pg!();
    let adapter = adapter(&uri);
    let ctx = reader("module:reuse::core");
    let mut pids = HashSet::new();
    for _ in 0..20 {
        pids.insert(value(&adapter, &ctx, "SELECT pg_backend_pid()::text").await);
    }
    assert_eq!(pids.len(), 1, "one backend served all 20 statements");
    let status = adapter.pool_status().unwrap();
    let pool = pool_of(&status, &ctx);
    assert_eq!(pool.created, 1);
    assert_eq!(pool.reused, 19);
    assert_eq!(pool.statement_cache_hits, 19);
    assert_eq!(pool.statement_cache_misses, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_load_stays_within_the_pool_limit() {
    let uri = require_pg!();
    let adapter = Arc::new(adapter_with(
        &uri,
        PoolSettings {
            max_per_pool: 3,
            ..PoolSettings::default()
        },
    ));
    let ctx = reader("module:limit::core");
    let calls: Vec<_> = (0..20)
        .map(|_| {
            let adapter = Arc::clone(&adapter);
            let ctx = ctx.clone();
            tokio::spawn(async move {
                value(
                    &adapter,
                    &ctx,
                    "SELECT pg_backend_pid()::text FROM pg_sleep(0.05)",
                )
                .await
            })
        })
        .collect();
    let mut pids = HashSet::new();
    for call in calls {
        pids.insert(call.await.unwrap());
    }
    let status = adapter.pool_status().unwrap();
    let pool = pool_of(&status, &ctx);
    assert!(pids.len() <= 3, "{} backends", pids.len());
    assert!(pool.created <= 3);
    assert!(pool.open <= 3);
    assert_eq!(pool.in_use, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkout_waits_and_then_times_out_cleanly() {
    let uri = require_pg!();
    let adapter = Arc::new(adapter_with(
        &uri,
        PoolSettings {
            max_per_pool: 1,
            wait_timeout: Duration::from_millis(200),
            ..PoolSettings::default()
        },
    ));
    let ctx = reader("module:wait::core");
    let busy = {
        let adapter = Arc::clone(&adapter);
        let ctx = ctx.clone();
        tokio::spawn(async move { value(&adapter, &ctx, "SELECT 'done' FROM pg_sleep(0.8)").await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    let err = adapter
        .prepared_query_in(&ctx, "SELECT 1", &[])
        .await
        .unwrap_err();
    let waited = started.elapsed();
    assert!(
        err.to_string().contains("no database connection free"),
        "{err}"
    );
    assert!(waited >= Duration::from_millis(180) && waited < Duration::from_millis(600));
    assert_eq!(busy.await.unwrap(), "done");
    // The slot is free again.
    assert_eq!(value(&adapter, &ctx, "SELECT 'again'").await, "again");
    let status = adapter.pool_status().unwrap();
    assert_eq!(pool_of(&status, &ctx).wait_timeouts, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pool_recovers_after_a_postgres_restart() {
    let uri = require_pg!();
    // Restarting the server breaks every other test's connections: run it
    // on its own (`FENRIR_PG_TEST_RESTART=1 cargo test --lib restart`).
    let (Ok(pg_ctl), Ok(data), Ok(_)) = (
        std::env::var("FENRIR_PG_TEST_PGCTL"),
        std::env::var("FENRIR_PG_TEST_DATA"),
        std::env::var("FENRIR_PG_TEST_RESTART"),
    ) else {
        eprintln!("skipped: FENRIR_PG_TEST_PGCTL / _DATA / _RESTART not set");
        return;
    };
    let adapter = adapter(&uri);
    let read = reader("module:restart::core");
    let write = writer("module:restart::core");
    let name = table("restart");
    adapter
        .simple_query(&format!("CREATE TABLE {name} (id int)"))
        .await
        .unwrap();
    let before = value(&adapter, &read, "SELECT pg_backend_pid()::text").await;
    let status = std::process::Command::new(&pg_ctl)
        .args(["restart", "-D", &data, "-m", "fast", "-w"])
        .env("LC_ALL", "C")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "pg_ctl restart failed");
    // A read right away: the pooled connection is dead and is replaced.
    let after = value(&adapter, &read, "SELECT pg_backend_pid()::text").await;
    assert_ne!(before, after);
    // A write on the same pool works too, without any host restart.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let written = adapter
        .prepared_execute_in(&write, &format!("INSERT INTO {name} VALUES (1)"), &[])
        .await
        .unwrap();
    assert_eq!(written, 1);
    adapter
        .simple_query(&format!("DROP TABLE {name}"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_statement_leaves_no_transaction_behind() {
    let uri = require_pg!();
    let adapter = adapter_with(
        &uri,
        PoolSettings {
            max_per_pool: 1,
            ..PoolSettings::default()
        },
    );
    let name = table("fail");
    adapter
        .simple_query(&format!("CREATE TABLE {name} (id int PRIMARY KEY)"))
        .await
        .unwrap();
    let write = writer("module:fail::core");
    let read = reader("module:fail::core");
    let insert = format!("INSERT INTO {name} VALUES (1)");
    adapter
        .prepared_execute_in(&write, &insert, &[])
        .await
        .unwrap();
    let pid = value(&adapter, &read, "SELECT pg_backend_pid()::text").await;
    // Unique violation inside the implicit transaction.
    assert!(adapter
        .prepared_execute_in(&write, &insert, &[])
        .await
        .is_err());
    // A failing read inside the read-only transaction.
    assert!(adapter
        .prepared_query_in(&read, "SELECT 1 / 0", &[])
        .await
        .is_err());
    // Same connection, nothing open: now() is this statement's own
    // transaction start, and the table holds exactly the first row.
    assert_eq!(
        value(&adapter, &read, "SELECT pg_backend_pid()::text").await,
        pid
    );
    assert_no_open_transaction(&adapter, &write).await;
    assert_eq!(
        value(
            &adapter,
            &read,
            &format!("SELECT count(*)::text FROM {name}")
        )
        .await,
        "1"
    );
    adapter
        .simple_query(&format!("DROP TABLE {name}"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_state_does_not_survive_a_call() {
    let uri = require_pg!();
    let adapter = adapter_with(
        &uri,
        PoolSettings {
            max_per_pool: 1,
            ..PoolSettings::default()
        },
    );
    let dirty = module(
        "module:state::core",
        DbAccess::ReadWrite,
        DbSessionEffect::Resettable,
    );
    let read = reader("module:state::core");
    let pid = value(&adapter, &read, "SELECT pg_backend_pid()::text").await;
    let default_path = value(&adapter, &read, "SELECT current_setting('search_path')").await;
    for sql in [
        "BEGIN",
        "SET search_path = pg_catalog",
        "SET application_name = 'leak'",
        "CREATE TEMP TABLE hp_leak (a int)",
        "SELECT pg_advisory_lock(4242)",
    ] {
        adapter.simple_query_in(&dirty, sql).await.unwrap();
    }
    // Same backend (the pool kept the connection), but clean.
    assert_eq!(
        value(&adapter, &read, "SELECT pg_backend_pid()::text").await,
        pid
    );
    assert_eq!(
        value(&adapter, &read, "SELECT current_setting('search_path')").await,
        default_path
    );
    assert_ne!(
        value(
            &adapter,
            &read,
            "SELECT current_setting('application_name')"
        )
        .await,
        "leak"
    );
    assert_eq!(
        value(
            &adapter,
            &read,
            "SELECT count(*)::text FROM pg_class WHERE relname = 'hp_leak'"
        )
        .await,
        "0"
    );
    assert_eq!(
        value(
            &adapter,
            &read,
            "SELECT count(*)::text FROM pg_locks WHERE locktype = 'advisory' AND pid = pg_backend_pid()"
        )
        .await,
        "0"
    );
    assert_no_open_transaction(&adapter, &writer("module:state::core")).await;
    // A statement that touches prepared statements is not reused at all.
    let once = module(
        "module:state::core",
        DbAccess::ReadWrite,
        DbSessionEffect::Unrecoverable,
    );
    adapter
        .simple_query_in(&once, "PREPARE hp_p AS SELECT 1")
        .await
        .unwrap();
    assert_ne!(
        value(&adapter, &read, "SELECT pg_backend_pid()::text").await,
        pid
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn modules_and_roles_never_share_connections() {
    let uri = require_pg!();
    let adapter = Arc::new(adapter(&uri));
    let mut by_key: Vec<HashSet<String>> = Vec::new();
    for name in ["module:a::core", "module:b::core"] {
        let ctx = reader(name);
        let calls: Vec<_> = (0..12)
            .map(|_| {
                let adapter = Arc::clone(&adapter);
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    value(
                        &adapter,
                        &ctx,
                        "SELECT pg_backend_pid()::text FROM pg_sleep(0.01)",
                    )
                    .await
                })
            })
            .collect();
        let mut pids = HashSet::new();
        for call in calls {
            pids.insert(call.await.unwrap());
        }
        by_key.push(pids);
    }
    // Again for a, after b ran: still only a's own backends.
    let again = value(
        &adapter,
        &reader("module:a::core"),
        "SELECT pg_backend_pid()::text",
    )
    .await;
    assert!(by_key[0].contains(&again));
    assert!(by_key[0].is_disjoint(&by_key[1]));

    // Another role is another adapter (one URI each), hence another pool.
    let role = table("role");
    let password = format!("pw{}", table("x"));
    adapter
        .simple_query(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'"))
        .await
        .unwrap();
    let base: tokio_postgres::Config = uri.parse().unwrap();
    let user = base.get_user().unwrap().to_string();
    let other_uri = uri.replacen(&user, &role, 1).replacen(
        &String::from_utf8_lossy(base.get_password().unwrap()).to_string(),
        &password,
        1,
    );
    let other = adapter_with(&other_uri, PoolSettings::default());
    let ctx = reader("module:a::core");
    assert_eq!(value(&other, &ctx, "SELECT current_user::text").await, role);
    assert_eq!(
        value(&adapter, &ctx, "SELECT current_user::text").await,
        user
    );
    drop(other);
    tokio::time::sleep(Duration::from_millis(100)).await;
    adapter
        .simple_query(&format!("DROP ROLE {role}"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_schema_change_invalidates_the_cached_statement() {
    let uri = require_pg!();
    let adapter = adapter_with(
        &uri,
        PoolSettings {
            max_per_pool: 1,
            ..PoolSettings::default()
        },
    );
    let name = table("schema");
    adapter
        .simple_query(&format!(
            "CREATE TABLE {name} (a int); INSERT INTO {name} VALUES (1)"
        ))
        .await
        .unwrap();
    let read = reader("module:schema::core");
    let select = format!("SELECT * FROM {name}");
    let columns = |results: Vec<DbExecutionResult>| match results.into_iter().next() {
        Some(DbExecutionResult::ResultSet(set)) => set.columns,
        other => panic!("{other:?}"),
    };
    let before = columns(
        adapter
            .prepared_query_in(&read, &select, &[])
            .await
            .unwrap(),
    );
    assert_eq!(before, vec!["a".to_string()]);
    // A migration adds a column: the cached plan's result type changes.
    adapter
        .simple_query(&format!("ALTER TABLE {name} ADD COLUMN b int"))
        .await
        .unwrap();
    let after = columns(
        adapter
            .prepared_query_in(&read, &select, &[])
            .await
            .unwrap(),
    );
    assert_eq!(after, vec!["a".to_string(), "b".to_string()]);
    let status = adapter.pool_status().unwrap();
    let pool = pool_of(&status, &read);
    assert!(pool.statement_cache_invalidations >= 1);
    assert_eq!(pool.retries, 1);
    // Also for a write on a cached statement whose result changed.
    let returning = format!("UPDATE {name} SET a = 2 RETURNING *");
    let write = writer("module:schema::core");
    adapter
        .prepared_query_in(&write, &returning, &[])
        .await
        .unwrap();
    adapter
        .simple_query(&format!("ALTER TABLE {name} ADD COLUMN c int"))
        .await
        .unwrap();
    let rows = adapter
        .prepared_query_in(&write, &returning, &[])
        .await
        .unwrap();
    assert_eq!(columns(rows).len(), 3);
    // The write ran once per call: a = 2 for the one row, no duplicates.
    assert_eq!(
        value(
            &adapter,
            &read,
            &format!("SELECT count(*)::text FROM {name}")
        )
        .await,
        "1"
    );
    adapter.invalidate_statement_caches();
    adapter
        .simple_query(&format!("DROP TABLE {name}"))
        .await
        .unwrap();
}

/// Task 7, defence in depth: a read runs in a read-only transaction, so a
/// function that writes fails even when the classification let it through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_run_in_a_read_only_transaction() {
    let uri = require_pg!();
    let adapter = adapter(&uri);
    let name = table("ro");
    adapter
        .simple_query(&format!(
            "CREATE TABLE {name} (a int); INSERT INTO {name} VALUES (1); \
             CREATE FUNCTION {name}_bump() RETURNS int LANGUAGE sql AS \
             $$ UPDATE {name} SET a = a + 1 RETURNING a $$"
        ))
        .await
        .unwrap();
    let read = reader("module:ro::core");
    let err = adapter
        .prepared_query_in(&read, &format!("SELECT {name}_bump()"), &[])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("read-only transaction"), "{err}");
    let err = adapter
        .prepared_query_in(
            &read,
            &format!("WITH x AS (UPDATE {name} SET a = 9 RETURNING a) SELECT * FROM x"),
            &[],
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("read-only transaction"), "{err}");
    assert!(adapter
        .simple_query_in(&read, &format!("SELECT {name}_bump()"))
        .await
        .is_err());
    assert_eq!(
        value(&adapter, &read, &format!("SELECT a::text FROM {name}")).await,
        "1"
    );
    // With write access the same CTE works and returns its rows.
    let write = writer("module:ro::core");
    let rows = adapter
        .prepared_query_in(
            &write,
            &format!("WITH x AS (UPDATE {name} SET a = 9 RETURNING a) SELECT a::text FROM x"),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(first_value(&rows), "9");
    adapter
        .simple_query(&format!(
            "DROP TABLE {name} CASCADE; DROP FUNCTION {name}_bump()"
        ))
        .await
        .unwrap();
}

/// Measurement, not a gate: statements per second and latency through the
/// pool with 1, 5 and 20 parallel callers.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "measurement"]
async fn measure_pool_concurrency() {
    let uri = require_pg!();
    let adapter = Arc::new(adapter(&uri));
    for callers in [1usize, 5, 20] {
        let ctx = reader("module:bench::core");
        let window = Duration::from_secs(3);
        let started = Instant::now();
        let tasks: Vec<_> = (0..callers)
            .map(|_| {
                let adapter = Arc::clone(&adapter);
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    let mut latencies = Vec::new();
                    while started.elapsed() < window {
                        let t = Instant::now();
                        adapter
                            .prepared_query_in(
                                &ctx,
                                "SELECT id, title FROM (VALUES (1, 'a')) v(id, title) WHERE id = 1",
                                &[],
                            )
                            .await
                            .unwrap();
                        latencies.push(t.elapsed());
                    }
                    latencies
                })
            })
            .collect();
        let mut all = Vec::new();
        for task in tasks {
            all.extend(task.await.unwrap());
        }
        all.sort();
        let p50 = all[all.len() / 2];
        let p95 = all[all.len() * 95 / 100];
        println!(
            "pool callers={callers:>2} stmts/s={:>8.0} p50={:>6.3}ms p95={:>6.3}ms",
            all.len() as f64 / window.as_secs_f64(),
            p50.as_secs_f64() * 1000.0,
            p95.as_secs_f64() * 1000.0
        );
    }
}

/// Through the port's plain methods (the host's own callers, and the only
/// API before 5.6): one backend for consecutive statements. Before 5.6 every
/// statement opened its own connection, so this failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plain_port_calls_reuse_one_backend() {
    let uri = require_pg!();
    let adapter = PostgresAdapter::new(&uri, None, None).unwrap();
    let mut pids = HashSet::new();
    for _ in 0..5 {
        let results = adapter
            .prepared_query("SELECT pg_backend_pid()::text", &[])
            .await
            .unwrap();
        pids.insert(first_value(&results));
    }
    assert_eq!(pids.len(), 1, "{} backends for 5 statements", pids.len());
}

/// S3 (5.6): module connections carry a `statement_timeout`, set when the
/// connection opens, so the session reset keeps it; the host's own pool has
/// none (migrations may run long).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn module_connections_carry_a_statement_timeout_that_survives_the_reset() {
    let uri = require_pg!();
    let adapter = adapter_with(
        &uri,
        PoolSettings {
            max_per_pool: 1,
            module_statement_timeout: Some(Duration::from_millis(300)),
            ..PoolSettings::default()
        },
    );
    let read = reader("module:timeout::core");
    let setting = "SELECT current_setting('statement_timeout')";
    assert_eq!(value(&adapter, &read, setting).await, "300ms");
    let host = adapter.prepared_query(setting, &[]).await.unwrap();
    assert_eq!(first_value(&host), "0");
    // Over the limit: the server cancels it, the connection stays usable.
    let pid = value(&adapter, &read, "SELECT pg_backend_pid()::text").await;
    let err = adapter
        .prepared_query_in(&read, "SELECT pg_sleep(2)", &[])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("statement timeout"), "{err}");
    assert_eq!(
        value(&adapter, &read, "SELECT pg_backend_pid()::text").await,
        pid
    );
    // A module's own SET lasts only until the reset.
    let dirty = module(
        "module:timeout::core",
        DbAccess::ReadWrite,
        DbSessionEffect::Resettable,
    );
    adapter
        .simple_query_in(&dirty, "SET statement_timeout = 0")
        .await
        .unwrap();
    assert_eq!(value(&adapter, &read, setting).await, "300ms");
    assert_eq!(
        value(&adapter, &read, "SELECT pg_backend_pid()::text").await,
        pid
    );
    // Off: no timeout for module connections either.
    let off = adapter_with(
        &uri,
        PoolSettings {
            module_statement_timeout: None,
            ..PoolSettings::default()
        },
    );
    assert_eq!(value(&off, &read, setting).await, "0");
}
