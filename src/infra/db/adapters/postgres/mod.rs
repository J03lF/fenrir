use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::time;
use tokio_postgres::{NoTls, SimpleQueryMessage};

use crate::domain::db::{
    DbAdminPort, DbColumn, DbError, DbExecutionResult, DbResult, DbResultSet, DbTable, DbTableKind,
    DbTableSchema,
};

pub struct PostgresAdapter {
    uri: Arc<String>,
    query_timeout: Option<Duration>,
}

impl PostgresAdapter {
    pub fn new(uri: &str, _pool_max: Option<u32>, timeout_ms: Option<u64>) -> Result<Self> {
        let uri = uri.trim();
        if uri.is_empty() {
            anyhow::bail!("postgres uri must not be empty");
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
            .map_err(|err| DbError::connection(format!("ungültige Postgres-Config: {err}")))?;
        let (client, connection) = {
            let fut = config.connect(NoTls);
            if let Some(timeout) = self.query_timeout {
                match time::timeout(timeout, fut).await {
                    Ok(inner) => inner.map_err(DbError::connection)?,
                    Err(_) => return Err(DbError::connection("Verbindungs-Timeout")),
                }
            } else {
                fut.await.map_err(DbError::connection)?
            }
        };
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
        let messages = if let Some(timeout) = self.query_timeout {
            match time::timeout(timeout, fut).await {
                Ok(inner) => inner.map_err(DbError::query)?,
                Err(_) => return Err(DbError::query("Query-Timeout")),
            }
        } else {
            fut.await.map_err(DbError::query)?
        };
        Ok(messages)
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

#[async_trait::async_trait]
impl DbAdminPort for PostgresAdapter {
    async fn ping(&self) -> DbResult<()> {
        let messages = self.exec_simple_query("SELECT 1;").await?;
        if messages.is_empty() {
            return Err(DbError::query("leere Antwort auf SELECT 1"));
        }
        Ok(())
    }

    async fn simple_query(&self, statement: &str) -> DbResult<Vec<DbExecutionResult>> {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            return Err(DbError::invalid_input("Statement darf nicht leer sein"));
        }
        let messages = self.exec_simple_query(trimmed).await?;
        Self::map_simple_messages(messages)
    }

    async fn list_tables(&self) -> DbResult<Vec<DbTable>> {
        const SQL: &str = "SELECT table_schema, table_name, table_type FROM information_schema.tables \
            WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY table_schema, table_name";
        let client = self.connect().await?;
        let fut = client.query(SQL, &[]);
        let rows = if let Some(timeout) = self.query_timeout {
            match time::timeout(timeout, fut).await {
                Ok(inner) => inner.map_err(DbError::query)?,
                Err(_) => return Err(DbError::query("Query-Timeout")),
            }
        } else {
            fut.await.map_err(DbError::query)?
        };
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
        let params: [&(dyn tokio_postgres::types::ToSql + Sync); 2] = [&schema_owned, &name_owned];
        let fut = client.query(SQL, &params);
        let rows = if let Some(timeout) = self.query_timeout {
            match time::timeout(timeout, fut).await {
                Ok(inner) => inner.map_err(DbError::query)?,
                Err(_) => return Err(DbError::query("Query-Timeout")),
            }
        } else {
            fut.await.map_err(DbError::query)?
        };
        if rows.is_empty() {
            return Err(DbError::InvalidInput {
                message: format!("Tabelle {}.{} nicht gefunden", schema_owned, name_owned),
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
}

fn split_table_reference(input: &str) -> (&str, &str) {
    let trimmed = input.trim().trim_matches('"');
    if let Some((schema, name)) = trimmed.split_once('.') {
        (schema.trim_matches('"'), name.trim_matches('"'))
    } else {
        ("public", trimmed)
    }
}
