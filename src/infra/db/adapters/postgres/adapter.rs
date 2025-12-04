use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64_ENGINE, Engine as _};
use tokio::time;
use tokio_postgres::{types::ToSql, NoTls, Row, SimpleQueryMessage};

use crate::domain::db::{
    DbAdminPort, DbColumn, DbError, DbExecutionResult, DbResult, DbResultSet, DbTable, DbTableKind,
    DbTableSchema, DbValue,
};
use crate::utils::messages::infra::db as infra_db_messages;

pub struct PostgresAdapter {
    uri: Arc<String>,
    query_timeout: Option<Duration>,
}

impl PostgresAdapter {
    pub fn new(uri: &str, _pool_max: Option<u32>, timeout_ms: Option<u64>) -> Result<Self> {
        let uri = uri.trim();
        if uri.is_empty() {
            anyhow::bail!(infra_db_messages::postgres::uri_empty());
        }
        let timeout = timeout_ms.map(Duration::from_millis);
        Ok(Self {
            uri: Arc::new(uri.to_string()),
            query_timeout: timeout,
        })
    }

    async fn connect(&self) -> DbResult<tokio_postgres::Client> {
        let config: tokio_postgres::Config = self
            .uri
            .parse()
            .map_err(|err| DbError::connection(infra_db_messages::postgres::invalid_config(err)))?;
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

    fn prepare_params(params: &[DbValue]) -> Vec<PreparedParamBinding> {
        params
            .iter()
            .map(|value| match value {
                DbValue::Null => PreparedParamBinding::Null(None),
                DbValue::Text(text) => PreparedParamBinding::Text(text.clone()),
                DbValue::Integer(num) => PreparedParamBinding::Integer(*num),
                DbValue::Float(num) => PreparedParamBinding::Float(*num),
                DbValue::Bool(flag) => PreparedParamBinding::Bool(*flag),
                DbValue::Json(json) => PreparedParamBinding::Json(json.clone()),
            })
            .collect()
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
        if let Ok(value) = row.try_get::<usize, i64>(idx) {
            return value.to_string();
        }
        if let Ok(value) = row.try_get::<usize, f64>(idx) {
            return value.to_string();
        }
        if let Ok(value) = row.try_get::<usize, bool>(idx) {
            return value.to_string();
        }
        if let Ok(value) = row.try_get::<usize, Vec<u8>>(idx) {
            return BASE64_ENGINE.encode(value);
        }
        "<unsupported>".to_string()
    }
}

enum PreparedParamBinding {
    Null(Option<String>),
    Text(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Json(String),
}

impl PreparedParamBinding {
    fn as_binding(&self) -> &(dyn ToSql + Sync) {
        match self {
            PreparedParamBinding::Null(value) => value,
            PreparedParamBinding::Text(value) => value,
            PreparedParamBinding::Integer(value) => value,
            PreparedParamBinding::Float(value) => value,
            PreparedParamBinding::Bool(value) => value,
            PreparedParamBinding::Json(value) => value,
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
        let bindings = Self::prepare_params(params);
        let refs: Vec<&(dyn ToSql + Sync)> = bindings
            .iter()
            .map(|binding| binding.as_binding())
            .collect();
        let fut = client.query(statement, &refs);
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
        let bindings = Self::prepare_params(params);
        let refs: Vec<&(dyn ToSql + Sync)> = bindings
            .iter()
            .map(|binding| binding.as_binding())
            .collect();
        let fut = client.execute(statement, &refs);
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
