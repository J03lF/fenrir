use std::sync::Arc;

use anyhow::Result;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rusqlite::{params_from_iter, types::ValueRef, Connection, Row};
use tokio::task;

use crate::domain::db::{
    DbAdminPort, DbColumn, DbError, DbExecutionResult, DbResult, DbResultSet, DbTable, DbTableKind,
    DbTableSchema, DbValue,
};
pub struct SqliteAdapter {
    uri: Arc<String>,
}

impl SqliteAdapter {
    pub fn new(uri: &str) -> Result<Self> {
        let uri = uri.trim();
        if uri.is_empty() {
            anyhow::bail!("sqlite uri must not be empty");
        }
        Ok(Self {
            uri: Arc::new(uri.to_string()),
        })
    }

    async fn with_conn<F, T>(&self, f: F) -> DbResult<T>
    where
        F: FnOnce(Connection) -> DbResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let path = self.uri.clone();
        task::spawn_blocking(move || {
            let conn = Connection::open(path.as_str())
                .map_err(|err| DbError::connection(format!("sqlite open failed: {err}")))?;
            f(conn)
        })
        .await
        .map_err(|err| DbError::Connection {
            message: err.to_string(),
        })?
    }

    fn map_rows(rows: Vec<Vec<String>>) -> Vec<DbExecutionResult> {
        if rows.is_empty() {
            return Vec::new();
        }
        let cols = rows
            .first()
            .map(|r| (0..r.len()).map(|i| format!("col{}", i + 1)).collect())
            .unwrap_or_default();
        vec![DbExecutionResult::ResultSet(DbResultSet::new(cols, rows))]
    }
}

#[async_trait::async_trait]
impl DbAdminPort for SqliteAdapter {
    async fn ping(&self) -> DbResult<()> {
        self.simple_query("SELECT 1;").await.map(|_| ())
    }

    async fn simple_query(&self, statement: &str) -> DbResult<Vec<DbExecutionResult>> {
        let sql = statement.trim();
        if sql.is_empty() {
            return Err(DbError::invalid_input("statement must not be empty"));
        }
        let sql_owned = sql.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(&sql_owned)
                .map_err(|err| DbError::query(err.to_string()))?;
            let rows_iter = stmt
                .query_map([], |row| Ok(row_to_strings(row)))
                .map_err(|err| DbError::query(err.to_string()))?;
            let mut rows = Vec::new();
            for row in rows_iter {
                rows.push(row.map_err(|err| DbError::query(err.to_string()))?);
            }
            Ok(Self::map_rows(rows))
        })
        .await
    }

    async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
        const SQL: &str = "SELECT name, type FROM sqlite_master WHERE type IN ('table','view') ORDER BY name";
        let sql = SQL.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|err| DbError::query(err.to_string()))?;
            let rows_iter = stmt
                .query_map([], |row| {
                    let name: String = row.get(0)?;
                    let ty: String = row.get(1)?;
                    Ok((name, ty))
                })
                .map_err(|err| DbError::query(err.to_string()))?;
            let mut out = Vec::new();
            for row in rows_iter {
                let (name, ty) = row.map_err(|err| DbError::query(err.to_string()))?;
                out.push(DbTable {
                    schema: None,
                    name: name.clone(),
                    kind: match ty.as_str() {
                        "table" => DbTableKind::Table,
                        "view" => DbTableKind::View,
                        _ => DbTableKind::Other,
                    },
                });
            }
            Ok(out)
        })
        .await
    }

    async fn describe_table(&self, table: &str) -> DbResult<DbTableSchema> {
        let table_name = table.trim();
        if table_name.is_empty() {
            return Err(DbError::invalid_input(
                format!("table {table} not found"),
            ));
        }
        let sql = format!("PRAGMA table_info('{table_name}')");
        let sql_owned = sql.clone();
        let columns = self
            .with_conn(move |conn| {
                let mut stmt = conn
                    .prepare(&sql_owned)
                    .map_err(|err| DbError::query(err.to_string()))?;
                let rows_iter = stmt
                    .query_map([], |row| {
                        let name: String = row.get(1)?;
                        let data_type: String = row.get(2)?;
                        let not_null: i32 = row.get(3)?;
                        let default_value: Option<String> = row.get(4)?;
                        Ok(DbColumn {
                            name,
                            data_type,
                            is_nullable: not_null == 0,
                            default_value,
                        })
                    })
                    .map_err(|err| DbError::query(err.to_string()))?;
                let mut cols = Vec::new();
                for row in rows_iter {
                    cols.push(row.map_err(|err| DbError::query(err.to_string()))?);
                }
                Ok(cols)
            })
            .await?;
        if columns.is_empty() {
            return Err(DbError::invalid_input(
                format!("table {table_name} not found"),
            ));
        }
        Ok(DbTableSchema {
            table: DbTable {
                schema: None,
                name: table_name.to_string(),
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
        let sql = statement.to_string();
        let params_vec = prepare_params(params)?;
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|err| DbError::query(err.to_string()))?;
            let rows_iter = stmt
                .query_map(params_from_iter(params_vec.iter()), |row| Ok(row_to_strings(row)))
                .map_err(|err| DbError::query(err.to_string()))?;
            let mut rows = Vec::new();
            for row in rows_iter {
                rows.push(row.map_err(|err| DbError::query(err.to_string()))?);
            }
            Ok(Self::map_rows(rows))
        })
        .await
    }

    async fn prepared_execute(&self, statement: &str, params: &[DbValue]) -> DbResult<u64> {
        let sql = statement.to_string();
        let params_vec = prepare_params(params)?;
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|err| DbError::query(err.to_string()))?;
            let affected = stmt
                .execute(params_from_iter(params_vec.iter()))
                .map_err(|err| DbError::query(err.to_string()))?;
            Ok(affected as u64)
        })
        .await
    }
}

fn row_to_strings(row: &Row) -> Vec<String> {
    let mut out = Vec::new();
    for idx in 0..row.as_ref().column_count() {
        let val = match row.get_ref(idx) {
            Ok(ValueRef::Null) => "NULL".to_string(),
            Ok(ValueRef::Integer(i)) => i.to_string(),
            Ok(ValueRef::Real(f)) => f.to_string(),
            Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).to_string(),
            Ok(ValueRef::Blob(b)) => STANDARD.encode(b),
            Err(_) => "<error>".to_string(),
        };
        out.push(val);
    }
    out
}

fn prepare_params(params: &[DbValue]) -> DbResult<Vec<rusqlite::types::Value>> {
    let mut out = Vec::with_capacity(params.len());
    for p in params {
        let v = match p {
            DbValue::Null => rusqlite::types::Value::Null,
            DbValue::Text(t) => rusqlite::types::Value::Text(t.clone()),
            DbValue::Integer(i) => rusqlite::types::Value::Integer(*i),
            DbValue::Float(f) => rusqlite::types::Value::Real(*f),
            DbValue::Bool(b) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
            DbValue::Json(j) => rusqlite::types::Value::Text(j.clone()),
            // SQLite stores timestamps as TEXT in ISO 8601 format
            DbValue::Timestamp(dt) => {
                let iso = dt
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| dt.to_string());
                rusqlite::types::Value::Text(iso)
            }
            DbValue::TimestampStr(s) => rusqlite::types::Value::Text(s.clone()),
        };
        out.push(v);
    }
    Ok(out)
}

