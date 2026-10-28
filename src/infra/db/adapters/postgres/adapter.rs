use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64_ENGINE, Engine as _};
use serde_json::{Number as JsonNumber, Value as JsonValue};
use tokio::time;
use tokio_postgres::{
    error::SqlState,
    types::{Json, ToSql, Type},
    Client, NoTls, Row, SimpleQueryMessage, Statement,
};

use super::pool::{ConnectFn, PgPool, PoolSettings, PooledConn};
use crate::domain::db::{
    DbAccess, DbAdminPort, DbCallContext, DbColumn, DbError, DbExecutionResult, DbPoolKey,
    DbPoolStatus, DbResult, DbResultSet, DbSessionEffect, DbTable, DbTableKind, DbTableSchema,
    DbValue,
};
use crate::utils::messages::infra::db as infra_db_messages;

/// Connection mode for PostgreSQL
#[derive(Debug, Clone)]
enum ConnectionMode {
    /// TCP connection (host:port)
    Tcp,
    /// Unix socket connection (socket directory path)
    UnixSocket(PathBuf),
}

/// Puts a connection back into a clean session without touching its
/// prepared statements (no `DISCARD ALL` / `DEALLOCATE`): ends any open or
/// failed transaction, resets settings and role, closes cursors, drops
/// `LISTEN`s, advisory locks, temporary tables and sequence caches.
const SESSION_RESET: &str = "ROLLBACK; RESET ALL; SET SESSION AUTHORIZATION DEFAULT; \
    CLOSE ALL; UNLISTEN *; SELECT pg_advisory_unlock_all(); DISCARD TEMP; DISCARD SEQUENCES";

/// Opens connections: parsed config plus how to reach the server.
#[derive(Clone)]
struct PgConnector {
    config: Arc<tokio_postgres::Config>,
    /// Config for module pools: `config` plus `-c statement_timeout=…` in
    /// the startup options, when a module timeout is set.
    module_config: Arc<tokio_postgres::Config>,
    mode: ConnectionMode,
}

impl PgConnector {
    fn new(config: tokio_postgres::Config, mode: ConnectionMode) -> Self {
        let config = Arc::new(config);
        Self {
            module_config: Arc::clone(&config),
            config,
            mode,
        }
    }

    /// Module connections start with `statement_timeout` set as a
    /// connection option. That makes it the session's default: the session
    /// reset (`RESET ALL`) keeps it, and a module's own `SET` lasts only
    /// until that reset.
    fn with_module_statement_timeout(mut self, timeout: Option<Duration>) -> Self {
        let Some(timeout) = timeout else {
            self.module_config = Arc::clone(&self.config);
            return self;
        };
        let mut config = (*self.config).clone();
        let setting = format!("-c statement_timeout={}", timeout.as_millis());
        let options = match config.get_options() {
            Some(existing) if !existing.trim().is_empty() => format!("{existing} {setting}"),
            _ => setting,
        };
        config.options(&options);
        self.module_config = Arc::new(config);
        self
    }

    /// The host's own pool never gets the module statement timeout.
    fn config_for(&self, key: &DbPoolKey) -> &tokio_postgres::Config {
        if *key == DbPoolKey::host() {
            &self.config
        } else {
            &self.module_config
        }
    }

    async fn connect(&self, key: &DbPoolKey) -> DbResult<Client> {
        let config = self.config_for(key);
        match &self.mode {
            ConnectionMode::Tcp => {
                let (client, connection) = config
                    .connect(NoTls)
                    .await
                    .map_err(|err| DbError::connection(PgErrorText(&err)))?;
                tokio::spawn(async move {
                    if let Err(err) = connection.await {
                        tracing::debug!(error = %err, "postgres connection terminated");
                    }
                });
                Ok(client)
            }
            ConnectionMode::UnixSocket(socket_dir) => {
                let socket = tokio::net::UnixStream::connect(self.socket_path(socket_dir))
                    .await
                    .map_err(|err| {
                        DbError::connection(format!(
                            "failed to connect to the Postgres Unix socket: {err}"
                        ))
                    })?;
                let (client, connection) =
                    config.connect_raw(socket, NoTls).await.map_err(|err| {
                        DbError::connection(format!(
                            "postgres handshake failed on Unix socket: {}",
                            PgErrorText(&err)
                        ))
                    })?;
                tokio::spawn(async move {
                    if let Err(err) = connection.await {
                        tracing::debug!(error = %err, "postgres unix socket connection terminated");
                    }
                });
                Ok(client)
            }
        }
    }

    /// PostgreSQL socket naming convention: `.s.PGSQL.<port>` (default 5432).
    fn socket_path(&self, socket_dir: &Path) -> PathBuf {
        let port = self.config.get_ports().first().copied().unwrap_or(5432);
        socket_dir.join(format!(".s.PGSQL.{port}"))
    }

    /// Best effort: ask the server to cancel what `client` is running (after
    /// a timeout, before the connection is dropped).
    fn cancel_in_background(&self, client: &Client) {
        let token = client.cancel_token();
        let connector = self.clone();
        tokio::spawn(async move {
            let result = match &connector.mode {
                ConnectionMode::Tcp => token.cancel_query(NoTls).await,
                ConnectionMode::UnixSocket(socket_dir) => {
                    match tokio::net::UnixStream::connect(connector.socket_path(socket_dir)).await {
                        Ok(stream) => token.cancel_query_raw(stream, NoTls).await,
                        Err(err) => {
                            tracing::debug!(error = %err, "postgres cancel: socket unreachable");
                            return;
                        }
                    }
                }
            };
            if let Err(err) = result {
                tracing::debug!(error = %err, "postgres cancel request failed");
            }
        });
    }
}

pub struct PostgresAdapter {
    query_timeout: Option<Duration>,
    connector: PgConnector,
    pool: PgPool,
}

/// What a prepared statement returns to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Rows,
    AffectedRows,
}

enum Executed {
    Rows(Vec<Row>),
    Affected(u64),
    Simple(Vec<SimpleQueryMessage>),
}

/// Why one attempt failed, and whether a second attempt is safe.
enum Failure {
    /// The cached statement no longer fits the schema (result type changed,
    /// statement gone). Raised at Bind, before anything ran: safe to retry
    /// for reads and writes.
    StalePlan(DbError),
    /// The connection broke or timed out. Nothing is known about a write.
    Connection(DbError),
    /// The server rejected the statement; the connection is fine.
    Statement(DbError),
}

impl Failure {
    fn into_error(self) -> DbError {
        match self {
            Failure::StalePlan(err) | Failure::Connection(err) | Failure::Statement(err) => err,
        }
    }
}

/// A tokio-postgres error as text, cause included: `db error: ERROR: …`.
/// Since tokio-postgres 0.7.14 `Display` stops at `db error`; the DB
/// connector hands these texts to the modules, so the old form stays.
struct PgErrorText<'a>(&'a tokio_postgres::Error);

impl fmt::Display for PgErrorText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)?;
        if let Some(cause) = std::error::Error::source(self.0) {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

fn pg_error(err: tokio_postgres::Error) -> DbError {
    DbError::query(PgErrorText(&err))
}

fn is_stale_plan(err: &tokio_postgres::Error) -> bool {
    err.code().is_some_and(|code| {
        (*code == SqlState::FEATURE_NOT_SUPPORTED
            && err.as_db_error().is_some_and(|db| {
                db.message()
                    .contains("cached plan must not change result type")
            }))
            || *code == SqlState::INVALID_SQL_STATEMENT_NAME
    })
}

/// Server errors that end the session: connection exceptions (class 08)
/// and operator intervention (57P01 admin shutdown, 57P02 crash shutdown,
/// 57P03 cannot connect now, 57P04 database dropped, 57P05 idle session
/// timeout).
fn ends_the_session(code: &SqlState) -> bool {
    let code = code.code();
    code.starts_with("08") || code.starts_with("57P")
}

fn classify_pg_error(err: tokio_postgres::Error) -> Failure {
    if is_stale_plan(&err) {
        return Failure::StalePlan(pg_error(err));
    }
    match err.code() {
        Some(code) if !ends_the_session(code) => Failure::Statement(pg_error(err)),
        _ => Failure::Connection(pg_error(err)),
    }
}

impl PostgresAdapter {
    pub fn new(uri: &str, pool_max: Option<u32>, timeout_ms: Option<u64>) -> Result<Self> {
        let uri = uri.trim();
        if uri.is_empty() {
            anyhow::bail!(infra_db_messages::postgres::uri_empty());
        }
        let mut settings = PoolSettings::default();
        if let Some(max) = pool_max.and_then(|max| usize::try_from(max).ok()) {
            if max > 0 {
                settings.max_per_pool = max;
                settings.max_total = settings.max_total.max(max);
            }
        }
        Self::with_settings(uri, settings, timeout_ms.map(Duration::from_millis))
    }

    pub(crate) fn with_settings(
        uri: &str,
        settings: PoolSettings,
        query_timeout: Option<Duration>,
    ) -> Result<Self> {
        let config: tokio_postgres::Config = uri
            .parse()
            .map_err(|err| anyhow::anyhow!(infra_db_messages::postgres::invalid_config(err)))?;
        let connector = PgConnector::new(config, Self::detect_connection_mode(uri))
            .with_module_statement_timeout(settings.module_statement_timeout);
        let connect = Self::connect_fn(&connector);
        Ok(Self {
            query_timeout,
            connector,
            pool: PgPool::new(settings, connect),
        })
    }

    fn connect_fn(connector: &PgConnector) -> ConnectFn {
        let connect_with = connector.clone();
        Arc::new(move |key: &DbPoolKey| {
            let connector = connect_with.clone();
            let key = key.clone();
            Box::pin(async move { connector.connect(&key).await })
        })
    }

    /// Sets the `statement_timeout` of module pools
    /// (`db.module_statement_timeout_ms`; `None` turns it off). Applies to
    /// connections opened afterwards, so call it right after construction.
    #[must_use]
    pub fn with_module_statement_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.connector = self.connector.with_module_statement_timeout(timeout);
        self.pool.set_connect(Self::connect_fn(&self.connector));
        self
    }

    /// Empties every pooled connection's statement cache (after migrations).
    pub fn invalidate_statement_caches(&self) {
        self.pool.invalidate_statement_caches();
    }

    /// Detect if the URI uses a Unix socket or TCP connection
    fn detect_connection_mode(uri: &str) -> ConnectionMode {
        // Parse the URI to check for Unix socket indicators
        // Format: postgres://user:pass@/dbname?host=/path/to/socket
        // Or: host=/path/to/socket in the query string
        if let Some(query_start) = uri.find('?') {
            let query = &uri[query_start + 1..];
            for param in query.split('&') {
                if let Some(value) = param.strip_prefix("host=") {
                    let path = urlencoding::decode(value)
                        .map(|s| s.into_owned())
                        .unwrap_or_else(|_| value.to_string());
                    // If host starts with '/' it's a Unix socket path
                    if path.starts_with('/') {
                        return ConnectionMode::UnixSocket(PathBuf::from(path));
                    }
                }
            }
        }
        ConnectionMode::Tcp
    }

    async fn await_pg<T, Fut, TimeoutFn, MapErr>(
        timeout: Option<Duration>,
        fut: Fut,
        timeout_error: TimeoutFn,
        map_err: MapErr,
    ) -> DbResult<T>
    where
        Fut: Future<Output = Result<T, tokio_postgres::Error>> + Send,
        TimeoutFn: FnOnce() -> DbError,
        MapErr: FnOnce(tokio_postgres::Error) -> DbError,
    {
        if let Some(limit) = timeout {
            match time::timeout(limit, fut).await {
                Ok(inner) => inner.map_err(map_err),
                Err(_) => Err(timeout_error()),
            }
        } else {
            fut.await.map_err(map_err)
        }
    }

    /// Runs `fut` with the query timeout. A timeout is a connection failure:
    /// the server is asked to cancel and the connection is not reused.
    async fn timed<T, Fut>(&self, client: &Client, fut: Fut) -> Result<T, Failure>
    where
        Fut: Future<Output = Result<T, Failure>>,
    {
        let Some(limit) = self.query_timeout else {
            return fut.await;
        };
        match time::timeout(limit, fut).await {
            Ok(result) => result,
            Err(_) => {
                self.connector.cancel_in_background(client);
                Err(Failure::Connection(DbError::query(
                    infra_db_messages::postgres::query_timeout(),
                )))
            }
        }
    }

    /// The cached prepared statement for `sql` on this connection, prepared
    /// on first use.
    async fn statement(&self, pooled: &mut PooledConn, sql: &str) -> Result<Statement, Failure> {
        let cached = pooled
            .conn()
            .map_err(Failure::Connection)?
            .statements
            .get(sql);
        pooled.record_cache(cached.is_some());
        if let Some(statement) = cached {
            return Ok(statement);
        }
        let conn = pooled.conn().map_err(Failure::Connection)?;
        let client = &conn.client;
        let statement = self
            .timed(client, async {
                client.prepare(sql).await.map_err(classify_pg_error)
            })
            .await?;
        pooled
            .conn()
            .map_err(Failure::Connection)?
            .statements
            .insert(sql, statement.clone());
        Ok(statement)
    }

    async fn prepared_once(
        &self,
        pooled: &mut PooledConn,
        ctx: &DbCallContext,
        sql: &str,
        params: &[DbValue],
        shape: Shape,
    ) -> Result<Executed, Failure> {
        let statement = self.statement(pooled, sql).await?;
        let bindings =
            Self::prepare_typed_params(params, statement.params()).map_err(Failure::Statement)?;
        let refs: Vec<&(dyn ToSql + Sync)> = bindings
            .iter()
            .map(|binding| binding.as_binding())
            .collect();
        let conn = pooled.conn().map_err(Failure::Connection)?;
        let client = &conn.client;
        let run = async {
            match shape {
                Shape::Rows => client.query(&statement, &refs).await.map(Executed::Rows),
                Shape::AffectedRows => client
                    .execute(&statement, &refs)
                    .await
                    .map(Executed::Affected),
            }
        };
        let result = match ctx.access {
            DbAccess::ReadWrite => {
                self.timed(client, async { run.await.map_err(classify_pg_error) })
                    .await
            }
            DbAccess::ReadOnly => self.timed(client, Self::read_only(client, run)).await,
        };
        if let Err(Failure::StalePlan(_)) = &result {
            conn.statements.remove(sql);
            pooled.record_invalidation();
        }
        result
    }

    async fn simple_once(
        &self,
        pooled: &mut PooledConn,
        ctx: &DbCallContext,
        sql: &str,
    ) -> Result<Executed, Failure> {
        let conn = pooled.conn().map_err(Failure::Connection)?;
        let client = &conn.client;
        let run = async { client.simple_query(sql).await.map(Executed::Simple) };
        match ctx.access {
            DbAccess::ReadWrite => {
                self.timed(client, async { run.await.map_err(classify_pg_error) })
                    .await
            }
            DbAccess::ReadOnly => self.timed(client, Self::read_only(client, run)).await,
        }
    }

    /// `run` inside `BEGIN READ ONLY` … `COMMIT`, pipelined into one round
    /// trip. A failed `run` makes the `COMMIT` a rollback; nothing is left
    /// open either way.
    async fn read_only<Fut>(client: &Client, run: Fut) -> Result<Executed, Failure>
    where
        Fut: Future<Output = Result<Executed, tokio_postgres::Error>>,
    {
        let (begin, result, commit) = tokio::join!(
            client.batch_execute("BEGIN READ ONLY"),
            run,
            client.batch_execute("COMMIT")
        );
        if let Err(err) = begin {
            return Err(Failure::Connection(pg_error(err)));
        }
        let executed = result.map_err(classify_pg_error)?;
        commit.map_err(|err| Failure::Connection(pg_error(err)))?;
        Ok(executed)
    }

    /// Leaves the connection clean for the next caller of its pool, or marks
    /// it for closing.
    async fn settle(
        &self,
        pooled: &mut PooledConn,
        ctx: &DbCallContext,
        failure: Option<&Failure>,
    ) {
        if matches!(failure, Some(Failure::Connection(_))) {
            pooled.discard();
            return;
        }
        match ctx.session {
            DbSessionEffect::None => {}
            DbSessionEffect::Unrecoverable => pooled.discard(),
            DbSessionEffect::Resettable => {
                let Ok(conn) = pooled.conn() else {
                    return;
                };
                let connection = conn.id;
                let reset = time::timeout(
                    Duration::from_secs(2),
                    conn.client.batch_execute(SESSION_RESET),
                )
                .await;
                pooled.record_reset();
                if !matches!(reset, Ok(Ok(()))) {
                    tracing::debug!(
                        connection,
                        "postgres session reset failed, closing the connection"
                    );
                    pooled.discard();
                }
            }
        }
    }

    /// One statement on behalf of `ctx`, through the pool. Retries once,
    /// on a new connection, when that is safe: a stale cached plan (nothing
    /// ran), or a broken reused connection for a read-only statement.
    async fn run(
        &self,
        ctx: &DbCallContext,
        sql: &str,
        params: Option<&[DbValue]>,
        shape: Shape,
    ) -> DbResult<Executed> {
        let mut attempt = 0u8;
        loop {
            attempt = attempt.saturating_add(1);
            let mut pooled = self.pool.get(&ctx.pool).await?;
            let outcome = match params {
                Some(params) => {
                    self.prepared_once(&mut pooled, ctx, sql, params, shape)
                        .await
                }
                None => self.simple_once(&mut pooled, ctx, sql).await,
            };
            match outcome {
                Ok(executed) => {
                    self.settle(&mut pooled, ctx, None).await;
                    return Ok(executed);
                }
                Err(failure) => {
                    let retry = attempt == 1
                        && match &failure {
                            Failure::StalePlan(_) => true,
                            Failure::Connection(_) => {
                                pooled.reused() && ctx.access == DbAccess::ReadOnly
                            }
                            Failure::Statement(_) => false,
                        };
                    self.settle(&mut pooled, ctx, Some(&failure)).await;
                    if retry {
                        pooled.record_retry();
                        drop(pooled);
                        continue;
                    }
                    return Err(failure.into_error());
                }
            }
        }
    }

    async fn exec_simple_query(
        &self,
        ctx: &DbCallContext,
        statement: &str,
    ) -> DbResult<Vec<SimpleQueryMessage>> {
        match self.run(ctx, statement, None, Shape::Rows).await? {
            Executed::Simple(messages) => Ok(messages),
            _ => Ok(Vec::new()),
        }
    }

    /// A host-internal query with typed rows (catalog lookups).
    async fn host_rows(&self, sql: &str, params: &[&(dyn ToSql + Sync)]) -> DbResult<Vec<Row>> {
        let mut pooled = self.pool.get(&DbPoolKey::host()).await?;
        let conn = pooled.conn()?;
        let result = Self::await_pg(
            self.query_timeout,
            conn.client.query(sql, params),
            || DbError::query(infra_db_messages::postgres::query_timeout()),
            pg_error,
        )
        .await;
        if result.is_err() {
            pooled.discard();
        }
        result
    }

    fn map_simple_messages(messages: Vec<SimpleQueryMessage>) -> DbResult<Vec<DbExecutionResult>> {
        let mut results = Vec::new();
        let mut current_columns: Option<Vec<String>> = None;
        let mut current_rows: Vec<Vec<String>> = Vec::new();

        for message in messages {
            match message {
                SimpleQueryMessage::RowDescription(columns) => {
                    current_columns = Some(columns.iter().map(|c| c.name().to_string()).collect());
                }
                SimpleQueryMessage::Row(row) => {
                    if current_columns.is_none() {
                        let columns = row
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect::<Vec<_>>();
                        current_columns = Some(columns);
                    }
                    let mut values = Vec::with_capacity(row.len());
                    for idx in 0..row.len() {
                        let value = row
                            .get(idx)
                            .map(|text| text.to_string())
                            .unwrap_or_else(|| "NULL".to_string());
                        values.push(value);
                    }
                    current_rows.push(values);
                }
                SimpleQueryMessage::CommandComplete(count) => {
                    let mut flushed = false;
                    if let Some(columns) = current_columns.take() {
                        let rows = std::mem::take(&mut current_rows);
                        results.push(DbExecutionResult::ResultSet(DbResultSet::new(
                            columns, rows,
                        )));
                        flushed = true;
                    }
                    if !flushed || count > 0 {
                        results.push(DbExecutionResult::AffectedRows(count));
                    }
                }
                _ => {}
            }
        }

        if let Some(columns) = current_columns.take() {
            let rows = std::mem::take(&mut current_rows);
            results.push(DbExecutionResult::ResultSet(DbResultSet::new(
                columns, rows,
            )));
        }

        Ok(results)
    }

    fn prepare_param(value: &DbValue) -> PreparedParamBinding {
        match value {
            DbValue::Null => PreparedParamBinding::NullText(None),
            DbValue::NullTimestamp => PreparedParamBinding::NullTimestamp(None),
            DbValue::NullUuid => PreparedParamBinding::NullUuid(None),
            DbValue::NullInet => PreparedParamBinding::NullInet(None),
            DbValue::Text(text) => PreparedParamBinding::Text(text.clone()),
            DbValue::TextArray(items) => PreparedParamBinding::TextArray(items.clone()),
            DbValue::Integer32(num) => PreparedParamBinding::Integer32(*num),
            DbValue::Integer(num) => PreparedParamBinding::Integer(*num),
            DbValue::Float(num) => PreparedParamBinding::Float(*num),
            DbValue::Bool(flag) => PreparedParamBinding::Bool(*flag),
            DbValue::Json(json) => PreparedParamBinding::Json(json.clone()),
            DbValue::Uuid(uuid) => PreparedParamBinding::Uuid(*uuid),
            DbValue::Inet(ip) => PreparedParamBinding::Inet(*ip),
            DbValue::Timestamp(dt) => PreparedParamBinding::Timestamp(*dt),
            DbValue::TimestampStr(s) => PreparedParamBinding::TimestampStr(s.clone()),
        }
    }

    fn prepare_params(params: &[DbValue]) -> Vec<PreparedParamBinding> {
        params.iter().map(Self::prepare_param).collect()
    }

    fn prepare_typed_params(
        params: &[DbValue],
        expected: &[Type],
    ) -> DbResult<Vec<PreparedParamBinding>> {
        if expected.len() != params.len() {
            return Ok(Self::prepare_params(params));
        }
        params
            .iter()
            .enumerate()
            .map(|(idx, value)| Self::coerce_param(value, &expected[idx]))
            .collect()
    }

    fn is_null_value(value: &DbValue) -> bool {
        matches!(
            value,
            DbValue::Null | DbValue::NullTimestamp | DbValue::NullUuid | DbValue::NullInet
        )
    }

    fn null_for_type(expected: &Type) -> PreparedParamBinding {
        match *expected {
            Type::INT2 => PreparedParamBinding::NullSmallInt(None),
            Type::INT4 => PreparedParamBinding::NullInt(None),
            Type::INT8 => PreparedParamBinding::NullBigInt(None),
            Type::BOOL => PreparedParamBinding::NullBool(None),
            Type::TIMESTAMPTZ => PreparedParamBinding::NullTimestamp(None),
            Type::UUID => PreparedParamBinding::NullUuid(None),
            Type::INET => PreparedParamBinding::NullInet(None),
            Type::JSON | Type::JSONB => PreparedParamBinding::NullJson(None),
            Type::TEXT_ARRAY | Type::VARCHAR_ARRAY | Type::BPCHAR_ARRAY | Type::NAME_ARRAY => {
                PreparedParamBinding::NullTextArray(None)
            }
            Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => {
                PreparedParamBinding::NullText(None)
            }
            _ => PreparedParamBinding::NullText(None),
        }
    }

    fn coerce_param(value: &DbValue, expected: &Type) -> DbResult<PreparedParamBinding> {
        if Self::is_null_value(value) {
            return Ok(Self::null_for_type(expected));
        }
        match *expected {
            Type::INT4 => Self::coerce_int4(value),
            Type::INT8 => Self::coerce_int8(value),
            Type::JSON | Type::JSONB => Self::coerce_json(value),
            Type::TEXT_ARRAY | Type::VARCHAR_ARRAY | Type::BPCHAR_ARRAY | Type::NAME_ARRAY => {
                Self::coerce_text_array(value)
            }
            Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => Self::coerce_text(value),
            Type::UUID => Self::coerce_uuid(value),
            _ => Ok(Self::prepare_param(value)),
        }
    }

    /// Coerce a value into a Postgres `uuid` binding.
    ///
    /// Modules ship UUIDs through the JSON-DB-Bridge as plain strings
    /// (`JsonValue::String(uuid_str)` → `DbValue::Text`). Without explicit
    /// coercion the driver would bind that as `text`, which the server
    /// rejects with *"cannot convert between String and uuid"* — even when
    /// the SQL contains an explicit `$N::uuid` cast, because tokio-postgres
    /// resolves the parameter type at PREPARE time before the cast runs.
    fn coerce_uuid(value: &DbValue) -> DbResult<PreparedParamBinding> {
        match value {
            DbValue::Uuid(uuid) => Ok(PreparedParamBinding::Uuid(*uuid)),
            DbValue::Text(text) => uuid::Uuid::parse_str(text.trim())
                .map(PreparedParamBinding::Uuid)
                .map_err(|err| {
                    DbError::invalid_input(format!(
                        "cannot parse '{}' as UUID parameter: {err}",
                        text
                    ))
                }),
            _ => Ok(Self::prepare_param(value)),
        }
    }

    #[cfg(test)]
    fn coerce_uuid_for_test(value: &DbValue) -> DbResult<PreparedParamBinding> {
        Self::coerce_uuid(value)
    }

    fn coerce_int4(value: &DbValue) -> DbResult<PreparedParamBinding> {
        match value {
            DbValue::Integer32(num) => Ok(PreparedParamBinding::Integer32(*num)),
            DbValue::Integer(num) => {
                if *num >= i32::MIN as i64 && *num <= i32::MAX as i64 {
                    Ok(PreparedParamBinding::Integer32(*num as i32))
                } else {
                    Err(DbError::invalid_input(format!(
                        "integer value {num} exceeds INT4 range"
                    )))
                }
            }
            DbValue::Text(text) => text
                .parse::<i32>()
                .map(PreparedParamBinding::Integer32)
                .map_err(|_| {
                    DbError::invalid_input(format!("cannot parse '{}' as INT4 parameter", text))
                }),
            _ => Ok(Self::prepare_param(value)),
        }
    }

    fn coerce_int8(value: &DbValue) -> DbResult<PreparedParamBinding> {
        match value {
            DbValue::Integer32(num) => Ok(PreparedParamBinding::Integer(*num as i64)),
            DbValue::Integer(num) => Ok(PreparedParamBinding::Integer(*num)),
            DbValue::Text(text) => text
                .parse::<i64>()
                .map(PreparedParamBinding::Integer)
                .map_err(|_| {
                    DbError::invalid_input(format!("cannot parse '{}' as INT8 parameter", text))
                }),
            _ => Ok(Self::prepare_param(value)),
        }
    }

    fn parse_json_value(text: &str) -> JsonValue {
        serde_json::from_str(text).unwrap_or_else(|_| JsonValue::String(text.to_string()))
    }

    fn coerce_json(value: &DbValue) -> DbResult<PreparedParamBinding> {
        let json_value = match value {
            DbValue::Json(text) => Self::parse_json_value(text),
            DbValue::Text(text) => Self::parse_json_value(text),
            DbValue::TextArray(items) => JsonValue::Array(
                items
                    .iter()
                    .map(|item| JsonValue::String(item.clone()))
                    .collect(),
            ),
            DbValue::Bool(flag) => JsonValue::Bool(*flag),
            DbValue::Integer(num) => JsonValue::Number(JsonNumber::from(*num)),
            DbValue::Integer32(num) => JsonValue::Number(JsonNumber::from(*num)),
            DbValue::Float(num) => {
                let Some(number) = JsonNumber::from_f64(*num) else {
                    return Err(DbError::invalid_input(format!(
                        "invalid float value for JSON parameter: {num}"
                    )));
                };
                JsonValue::Number(number)
            }
            DbValue::Uuid(uuid) => JsonValue::String(uuid.to_string()),
            DbValue::Inet(ip) => JsonValue::String(ip.to_string()),
            DbValue::Timestamp(dt) => JsonValue::String(dt.to_string()),
            DbValue::TimestampStr(text) => JsonValue::String(text.clone()),
            other => JsonValue::String(format!("{other:?}")),
        };
        Ok(PreparedParamBinding::JsonParam(Json(json_value)))
    }

    fn coerce_text_array(value: &DbValue) -> DbResult<PreparedParamBinding> {
        let json_value = match value {
            DbValue::TextArray(items) => return Ok(PreparedParamBinding::TextArray(items.clone())),
            DbValue::Json(text) => Self::parse_json_value(text),
            DbValue::Text(text) => {
                if text.trim().is_empty() {
                    JsonValue::Array(Vec::new())
                } else {
                    Self::parse_json_value(text)
                }
            }
            DbValue::Bool(flag) => JsonValue::Bool(*flag),
            DbValue::Integer(num) => JsonValue::Number(JsonNumber::from(*num)),
            DbValue::Integer32(num) => JsonValue::Number(JsonNumber::from(*num)),
            DbValue::Float(num) => {
                let Some(number) = JsonNumber::from_f64(*num) else {
                    return Err(DbError::invalid_input(format!(
                        "invalid float value for text array parameter: {num}"
                    )));
                };
                JsonValue::Number(number)
            }
            DbValue::Uuid(uuid) => JsonValue::String(uuid.to_string()),
            DbValue::Inet(ip) => JsonValue::String(ip.to_string()),
            DbValue::Timestamp(dt) => JsonValue::String(dt.to_string()),
            DbValue::TimestampStr(text) => JsonValue::String(text.clone()),
            other => JsonValue::String(format!("{other:?}")),
        };

        let items = match json_value {
            JsonValue::Array(values) => values
                .into_iter()
                .filter_map(|value| match value {
                    JsonValue::String(text) => Some(text),
                    JsonValue::Number(num) => Some(num.to_string()),
                    JsonValue::Bool(flag) => Some(flag.to_string()),
                    JsonValue::Null => None,
                    other => Some(other.to_string()),
                })
                .collect::<Vec<_>>(),
            JsonValue::String(text) => vec![text],
            JsonValue::Null => Vec::new(),
            other => vec![other.to_string()],
        };

        Ok(PreparedParamBinding::TextArray(items))
    }

    fn coerce_text(value: &DbValue) -> DbResult<PreparedParamBinding> {
        let text = match value {
            DbValue::Text(text) => text.clone(),
            DbValue::Json(text) => text.clone(),
            DbValue::Uuid(uuid) => uuid.to_string(),
            DbValue::Inet(ip) => ip.to_string(),
            DbValue::Timestamp(dt) => dt.to_string(),
            DbValue::TimestampStr(text) => text.clone(),
            DbValue::Bool(flag) => flag.to_string(),
            DbValue::Integer(num) => num.to_string(),
            DbValue::Integer32(num) => num.to_string(),
            DbValue::Float(num) => num.to_string(),
            DbValue::TextArray(items) => {
                serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string())
            }
            other => format!("{other:?}"),
        };
        Ok(PreparedParamBinding::Text(text))
    }

    fn map_rows(rows: Vec<Row>) -> DbResult<Vec<DbExecutionResult>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let columns = rows[0]
            .columns()
            .iter()
            .map(|col| col.name().to_string())
            .collect();
        let mut table = Vec::with_capacity(rows.len());
        for row in rows {
            let mut values = Vec::with_capacity(row.len());
            for idx in 0..row.len() {
                values.push(Self::stringify_row_value(&row, idx));
            }
            table.push(values);
        }
        Ok(vec![DbExecutionResult::ResultSet(DbResultSet::new(
            columns, table,
        ))])
    }

    fn stringify_row_value(row: &Row, idx: usize) -> String {
        if let Ok(value) = row.try_get::<usize, Option<String>>(idx) {
            return value.unwrap_or_else(|| "NULL".to_string());
        }
        if let Ok(value) = row.try_get::<usize, Option<Json<JsonValue>>>(idx) {
            return value
                .map(|json| json.0.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }
        if let Ok(value) = row.try_get::<usize, Option<uuid::Uuid>>(idx) {
            return value
                .map(|u| u.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }
        if let Ok(value) = row.try_get::<usize, Option<Vec<String>>>(idx) {
            if let Some(values) = value {
                if let Ok(json) = serde_json::to_string(&values) {
                    return json;
                }
            } else {
                return "NULL".to_string();
            }
        }
        if let Ok(value) = row.try_get::<usize, Option<Vec<Option<String>>>>(idx) {
            if let Some(values) = value {
                let normalized = values.into_iter().flatten().collect::<Vec<_>>();
                if let Ok(json) = serde_json::to_string(&normalized) {
                    return json;
                }
            } else {
                return "NULL".to_string();
            }
        }
        if let Ok(value) = row.try_get::<usize, Option<std::net::IpAddr>>(idx) {
            return value
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }
        if let Ok(value) = row.try_get::<usize, i64>(idx) {
            return value.to_string();
        }
        if let Ok(value) = row.try_get::<usize, i32>(idx) {
            return value.to_string();
        }
        if let Ok(value) = row.try_get::<usize, f64>(idx) {
            return value.to_string();
        }
        if let Ok(value) = row.try_get::<usize, bool>(idx) {
            return value.to_string();
        }
        // Timestamps - try OffsetDateTime first, then use to_string()
        if let Ok(value) = row.try_get::<usize, Option<::time::OffsetDateTime>>(idx) {
            return value
                .map(|dt| dt.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }
        if let Ok(value) = row.try_get::<usize, Vec<u8>>(idx) {
            return BASE64_ENGINE.encode(value);
        }
        "<unsupported>".to_string()
    }
}

enum PreparedParamBinding {
    NullText(Option<String>),
    NullSmallInt(Option<i16>),
    NullInt(Option<i32>),
    NullBigInt(Option<i64>),
    NullTimestamp(Option<::time::OffsetDateTime>),
    NullBool(Option<bool>),
    NullJson(Option<JsonValue>),
    NullTextArray(Option<Vec<String>>),
    NullUuid(Option<uuid::Uuid>),
    NullInet(Option<std::net::IpAddr>),
    Text(String),
    TextArray(Vec<String>),
    Integer32(i32),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Json(String),
    JsonParam(Json<JsonValue>),
    Uuid(uuid::Uuid),
    Inet(std::net::IpAddr),
    Timestamp(::time::OffsetDateTime),
    /// Timestamp as string - will be cast to TIMESTAMPTZ by Postgres
    TimestampStr(String),
}

impl PreparedParamBinding {
    fn as_binding(&self) -> &(dyn ToSql + Sync) {
        match self {
            PreparedParamBinding::NullText(value) => value,
            PreparedParamBinding::NullSmallInt(value) => value,
            PreparedParamBinding::NullInt(value) => value,
            PreparedParamBinding::NullBigInt(value) => value,
            PreparedParamBinding::NullTimestamp(value) => value,
            PreparedParamBinding::NullBool(value) => value,
            PreparedParamBinding::NullJson(value) => value,
            PreparedParamBinding::NullTextArray(value) => value,
            PreparedParamBinding::NullUuid(value) => value,
            PreparedParamBinding::NullInet(value) => value,
            PreparedParamBinding::Text(value) => value,
            PreparedParamBinding::TextArray(value) => value,
            PreparedParamBinding::Integer32(value) => value,
            PreparedParamBinding::Integer(value) => value,
            PreparedParamBinding::Float(value) => value,
            PreparedParamBinding::Bool(value) => value,
            PreparedParamBinding::Json(value) => value,
            PreparedParamBinding::JsonParam(value) => value,
            PreparedParamBinding::Uuid(value) => value,
            PreparedParamBinding::Inet(value) => value,
            PreparedParamBinding::Timestamp(value) => value,
            // For TimestampStr, we bind as text - the query should cast it
            PreparedParamBinding::TimestampStr(value) => value,
        }
    }
}

impl DbTableKind {
    fn from_pg(table_type: &str) -> Self {
        match table_type {
            "BASE TABLE" => DbTableKind::Table,
            "VIEW" => DbTableKind::View,
            "MATERIALIZED VIEW" => DbTableKind::MaterializedView,
            "INDEX" => DbTableKind::Index,
            _ => DbTableKind::Other,
        }
    }
}

fn split_table_reference(input: &str) -> (&str, &str) {
    let trimmed = input.trim().trim_matches('"');
    if let Some((schema, name)) = trimmed.split_once('.') {
        (schema.trim_matches('"'), name.trim_matches('"'))
    } else {
        ("public", trimmed)
    }
}

/// Host-internal catalog lookups leave nothing behind.
fn host_catalog_context() -> DbCallContext {
    DbCallContext {
        session: DbSessionEffect::None,
        ..DbCallContext::host()
    }
}

#[async_trait::async_trait]
impl DbAdminPort for PostgresAdapter {
    async fn ping(&self) -> DbResult<()> {
        let messages = self
            .exec_simple_query(&host_catalog_context(), "SELECT 1;")
            .await?;
        if messages.is_empty() {
            return Err(DbError::query(
                infra_db_messages::postgres::empty_ping_response(),
            ));
        }
        Ok(())
    }

    async fn simple_query(&self, statement: &str) -> DbResult<Vec<DbExecutionResult>> {
        self.simple_query_in(&DbCallContext::host(), statement)
            .await
    }

    async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
        const SQL: &str = "SELECT table_schema, table_name, table_type FROM information_schema.tables \
            WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY table_schema, table_name";
        let rows = self.host_rows(SQL, &[]).await?;
        let mut result = Vec::with_capacity(rows.len());
        for row in rows {
            let schema: String = row.get(0);
            let name: String = row.get(1);
            let ty: String = row.get(2);
            result.push(DbTable {
                schema: Some(schema.clone()),
                name: name.clone(),
                kind: DbTableKind::from_pg(&ty),
            });
        }
        Ok(result)
    }

    async fn describe_table(&self, table: &str) -> DbResult<DbTableSchema> {
        let (schema, name) = split_table_reference(table);
        const SQL: &str = "SELECT column_name, data_type, is_nullable, column_default \
            FROM information_schema.columns WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position";
        let schema_owned = schema.to_string();
        let name_owned = name.to_string();
        let schema_param: &(dyn ToSql + Sync) = &schema_owned;
        let name_param: &(dyn ToSql + Sync) = &name_owned;
        let rows = self.host_rows(SQL, &[schema_param, name_param]).await?;
        if rows.is_empty() {
            return Err(DbError::InvalidInput {
                message: infra_db_messages::postgres::table_not_found(&schema_owned, &name_owned),
            });
        }
        let mut columns = Vec::with_capacity(rows.len());
        for row in rows {
            let column_name: String = row.get(0);
            let data_type: String = row.get(1);
            let is_nullable: String = row.get(2);
            let default_value: Option<String> = row.get(3);
            columns.push(DbColumn {
                name: column_name,
                data_type,
                is_nullable: is_nullable.eq_ignore_ascii_case("YES"),
                default_value,
            });
        }
        Ok(DbTableSchema {
            table: DbTable {
                schema: Some(schema_owned),
                name: name_owned,
                kind: DbTableKind::Table,
            },
            columns,
        })
    }

    async fn prepared_query(
        &self,
        statement: &str,
        params: &[DbValue],
    ) -> DbResult<Vec<DbExecutionResult>> {
        self.prepared_query_in(&DbCallContext::host(), statement, params)
            .await
    }

    async fn prepared_execute(&self, statement: &str, params: &[DbValue]) -> DbResult<u64> {
        self.prepared_execute_in(&DbCallContext::host(), statement, params)
            .await
    }

    async fn simple_query_in(
        &self,
        ctx: &DbCallContext,
        statement: &str,
    ) -> DbResult<Vec<DbExecutionResult>> {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            return Err(DbError::invalid_input(
                infra_db_messages::postgres::statement_empty(),
            ));
        }
        let messages = self.exec_simple_query(ctx, trimmed).await?;
        Self::map_simple_messages(messages)
    }

    async fn prepared_query_in(
        &self,
        ctx: &DbCallContext,
        statement: &str,
        params: &[DbValue],
    ) -> DbResult<Vec<DbExecutionResult>> {
        match self.run(ctx, statement, Some(params), Shape::Rows).await? {
            Executed::Rows(rows) => Self::map_rows(rows),
            _ => Ok(Vec::new()),
        }
    }

    async fn prepared_execute_in(
        &self,
        ctx: &DbCallContext,
        statement: &str,
        params: &[DbValue],
    ) -> DbResult<u64> {
        match self
            .run(ctx, statement, Some(params), Shape::AffectedRows)
            .await?
        {
            Executed::Affected(count) => Ok(count),
            _ => Ok(0),
        }
    }

    fn pool_status(&self) -> Option<DbPoolStatus> {
        Some(self.pool.status())
    }
}

impl std::fmt::Debug for PreparedParamBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreparedParamBinding::Uuid(uuid) => write!(f, "Uuid({uuid})"),
            PreparedParamBinding::Text(text) => write!(f, "Text({text})"),
            other => write!(
                f,
                "{}",
                match other {
                    PreparedParamBinding::NullText(_) => "NullText",
                    PreparedParamBinding::NullSmallInt(_) => "NullSmallInt",
                    PreparedParamBinding::NullInt(_) => "NullInt",
                    PreparedParamBinding::NullBigInt(_) => "NullBigInt",
                    PreparedParamBinding::NullTimestamp(_) => "NullTimestamp",
                    PreparedParamBinding::NullBool(_) => "NullBool",
                    PreparedParamBinding::NullJson(_) => "NullJson",
                    PreparedParamBinding::NullTextArray(_) => "NullTextArray",
                    PreparedParamBinding::NullUuid(_) => "NullUuid",
                    PreparedParamBinding::NullInet(_) => "NullInet",
                    PreparedParamBinding::TextArray(_) => "TextArray",
                    PreparedParamBinding::Integer32(_) => "Integer32",
                    PreparedParamBinding::Integer(_) => "Integer",
                    PreparedParamBinding::Float(_) => "Float",
                    PreparedParamBinding::Bool(_) => "Bool",
                    PreparedParamBinding::Json(_) => "Json",
                    PreparedParamBinding::JsonParam(_) => "JsonParam",
                    PreparedParamBinding::Inet(_) => "Inet",
                    PreparedParamBinding::Timestamp(_) => "Timestamp",
                    PreparedParamBinding::TimestampStr(_) => "TimestampStr",
                    PreparedParamBinding::Uuid(_) | PreparedParamBinding::Text(_) => unreachable!(),
                }
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coerce_uuid_accepts_text_payload() {
        // JSON-DB-Bridge sends UUIDs as JSON strings. Without this coercion
        // tokio-postgres binds them as TEXT and the server rejects the bind.
        let raw = "0a8da3c6-2c61-4e34-9d7c-1f2d99e88d2f";
        let binding = PostgresAdapter::coerce_uuid_for_test(&DbValue::Text(raw.to_string()))
            .expect("text uuid must coerce");
        match binding {
            PreparedParamBinding::Uuid(uuid) => assert_eq!(uuid.to_string(), raw),
            other => panic!("expected PreparedParamBinding::Uuid, got {other:?}"),
        }
    }

    #[test]
    fn coerce_uuid_accepts_typed_uuid_payload() {
        let uuid = uuid::Uuid::new_v4();
        let binding = PostgresAdapter::coerce_uuid_for_test(&DbValue::Uuid(uuid))
            .expect("typed uuid must coerce");
        match binding {
            PreparedParamBinding::Uuid(out) => assert_eq!(out, uuid),
            other => panic!("expected PreparedParamBinding::Uuid, got {other:?}"),
        }
    }

    #[test]
    fn coerce_uuid_rejects_unparseable_text() {
        let err = PostgresAdapter::coerce_uuid_for_test(&DbValue::Text("not-a-uuid".into()))
            .expect_err("must reject garbage text");
        let msg = format!("{err:?}");
        assert!(msg.contains("UUID parameter"), "unexpected error: {msg}");
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/infra/db/postgres_pool_tests.rs"]
mod pool_tests;
