use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64_ENGINE, Engine as _};
use serde_json::{Number as JsonNumber, Value as JsonValue};
use tokio::time;
use tokio_postgres::{
    types::{Json, ToSql, Type},
    NoTls, Row, SimpleQueryMessage,
};

use crate::domain::db::{
    DbAdminPort, DbColumn, DbError, DbExecutionResult, DbResult, DbResultSet, DbTable, DbTableKind,
    DbTableSchema, DbValue,
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

pub struct PostgresAdapter {
    uri: Arc<String>,
    query_timeout: Option<Duration>,
    connection_mode: ConnectionMode,
}

impl PostgresAdapter {
    pub fn new(uri: &str, _pool_max: Option<u32>, timeout_ms: Option<u64>) -> Result<Self> {
        let uri = uri.trim();
        if uri.is_empty() {
            anyhow::bail!(infra_db_messages::postgres::uri_empty());
        }
        let timeout = timeout_ms.map(Duration::from_millis);

        // Detect if URI uses Unix socket (host parameter points to a directory)
        let connection_mode = Self::detect_connection_mode(uri);

        Ok(Self {
            uri: Arc::new(uri.to_string()),
            query_timeout: timeout,
            connection_mode,
        })
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

    async fn connect(&self) -> DbResult<tokio_postgres::Client> {
        let config: tokio_postgres::Config = self
            .uri
            .parse()
            .map_err(|err| DbError::connection(infra_db_messages::postgres::invalid_config(err)))?;

        match &self.connection_mode {
            ConnectionMode::Tcp => {
                // Standard TCP connection
                let (client, connection) = Self::await_pg(
                    self.query_timeout,
                    config.connect(NoTls),
                    || DbError::connection(infra_db_messages::postgres::connection_timeout()),
                    DbError::connection,
                )
                .await?;
                tokio::spawn(async move {
                    if let Err(err) = connection.await {
                        tracing::error!(error = %err, "postgres connection terminated");
                    }
                });
                Ok(client)
            }
            ConnectionMode::UnixSocket(socket_dir) => {
                // Unix socket connection
                self.connect_unix_socket(&config, socket_dir).await
            }
        }
    }

    /// Connect via Unix socket
    async fn connect_unix_socket(
        &self,
        config: &tokio_postgres::Config,
        socket_dir: &Path,
    ) -> DbResult<tokio_postgres::Client> {
        use tokio::net::UnixStream;

        // PostgreSQL socket naming convention: .s.PGSQL.<port>
        // Default port is 5432 if not specified
        let port = config.get_ports().first().copied().unwrap_or(5432);
        let socket_path = socket_dir.join(format!(".s.PGSQL.{}", port));

        // Connect to the Unix socket
        let socket = UnixStream::connect(&socket_path).await.map_err(|err| {
            DbError::connection(format!(
                "failed to connect to Unix socket at {}: {}",
                socket_path.display(),
                err
            ))
        })?;

        // Use connect_raw with the socket
        let (client, connection) = config.connect_raw(socket, NoTls).await.map_err(|err| {
            DbError::connection(format!("postgres handshake failed on Unix socket: {}", err))
        })?;

        tokio::spawn(async move {
            if let Err(err) = connection.await {
                tracing::error!(error = %err, "postgres unix socket connection terminated");
            }
        });

        Ok(client)
    }

    async fn exec_simple_query(&self, statement: &str) -> DbResult<Vec<SimpleQueryMessage>> {
        let client = self.connect().await?;
        let fut = client.simple_query(statement);
        let messages = Self::await_pg(
            self.query_timeout,
            fut,
            || DbError::query(infra_db_messages::postgres::query_timeout()),
            DbError::query,
        )
        .await?;
        Ok(messages)
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
            _ => Ok(Self::prepare_param(value)),
        }
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

#[async_trait::async_trait]
impl DbAdminPort for PostgresAdapter {
    async fn ping(&self) -> DbResult<()> {
        let messages = self.exec_simple_query("SELECT 1;").await?;
        if messages.is_empty() {
            return Err(DbError::query(
                infra_db_messages::postgres::empty_ping_response(),
            ));
        }
        Ok(())
    }

    async fn simple_query(&self, statement: &str) -> DbResult<Vec<DbExecutionResult>> {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            return Err(DbError::invalid_input(
                infra_db_messages::postgres::statement_empty(),
            ));
        }
        let messages = self.exec_simple_query(trimmed).await?;
        Self::map_simple_messages(messages)
    }

    async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
        const SQL: &str = "SELECT table_schema, table_name, table_type FROM information_schema.tables \
            WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY table_schema, table_name";
        let client = self.connect().await?;
        let empty_params: Vec<&(dyn ToSql + Sync)> = Vec::new();
        let fut = client.query(SQL, &empty_params);
        let rows = Self::await_pg(
            self.query_timeout,
            fut,
            || DbError::query(infra_db_messages::postgres::query_timeout()),
            DbError::query,
        )
        .await?;
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
        let client = self.connect().await?;
        const SQL: &str = "SELECT column_name, data_type, is_nullable, column_default \
            FROM information_schema.columns WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position";
        let schema_owned = schema.to_string();
        let name_owned = name.to_string();
        let schema_param: &(dyn ToSql + Sync) = &schema_owned;
        let name_param: &(dyn ToSql + Sync) = &name_owned;
        let params: Vec<&(dyn ToSql + Sync)> = vec![schema_param, name_param];
        let fut = client.query(SQL, &params);
        let rows = Self::await_pg(
            self.query_timeout,
            fut,
            || DbError::query(infra_db_messages::postgres::query_timeout()),
            DbError::query,
        )
        .await?;
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
        let client = self.connect().await?;
        let stmt = Self::await_pg(
            self.query_timeout,
            client.prepare(statement),
            || DbError::query(infra_db_messages::postgres::query_timeout()),
            DbError::query,
        )
        .await?;
        let bindings = Self::prepare_typed_params(params, stmt.params())?;
        let refs: Vec<&(dyn ToSql + Sync)> = bindings
            .iter()
            .map(|binding| binding.as_binding())
            .collect();
        let fut = client.query(&stmt, &refs);
        let rows = Self::await_pg(
            self.query_timeout,
            fut,
            || DbError::query(infra_db_messages::postgres::query_timeout()),
            DbError::query,
        )
        .await?;
        Self::map_rows(rows)
    }

    async fn prepared_execute(&self, statement: &str, params: &[DbValue]) -> DbResult<u64> {
        let client = self.connect().await?;
        let stmt = Self::await_pg(
            self.query_timeout,
            client.prepare(statement),
            || DbError::query(infra_db_messages::postgres::query_timeout()),
            DbError::query,
        )
        .await?;
        let bindings = Self::prepare_typed_params(params, stmt.params())?;
        let refs: Vec<&(dyn ToSql + Sync)> = bindings
            .iter()
            .map(|binding| binding.as_binding())
            .collect();
        let fut = client.execute(&stmt, &refs);
        let affected = Self::await_pg(
            self.query_timeout,
            fut,
            || DbError::query(infra_db_messages::postgres::query_timeout()),
            DbError::query,
        )
        .await?;
        Ok(affected)
    }
}
