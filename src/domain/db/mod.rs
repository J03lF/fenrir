use std::fmt;
use std::str::FromStr;

use async_trait::async_trait;

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DbEngine {
    Postgres,
    Mysql,
    Sqlite,
    Mongodb,
}

impl DbEngine {
    pub fn as_str(&self) -> &'static str {
        match self {
            DbEngine::Postgres => "postgres",
            DbEngine::Mysql => "mysql",
            DbEngine::Sqlite => "sqlite",
            DbEngine::Mongodb => "mongodb",
        }
    }
}

impl fmt::Display for DbEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DbEngine {
    type Err = DbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" => Ok(DbEngine::Postgres),
            "mysql" => Ok(DbEngine::Mysql),
            "sqlite" | "sqlite3" => Ok(DbEngine::Sqlite),
            "mongodb" | "mongo" => Ok(DbEngine::Mongodb),
            other => Err(DbError::InvalidInput {
                message: format!("unbekannter DB-Typ: {other}"),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl DbResultSet {
    pub fn new(columns: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self { columns, rows }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbExecutionResult {
    ResultSet(DbResultSet),
    AffectedRows(u64),
    CommandTag(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbTable {
    pub schema: Option<String>,
    pub name: String,
    pub kind: DbTableKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbTableKind {
    Table,
    View,
    MaterializedView,
    Index,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbColumn {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbTableSchema {
    pub table: DbTable,
    pub columns: Vec<DbColumn>,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum DbError {
    #[error("DB-Engine nicht konfiguriert: {engine}")]
    EngineNotConfigured { engine: DbEngine },
    #[error("DB-Verbindungsfehler: {message}")]
    Connection { message: String },
    #[error("DB-Abfragefehler: {message}")]
    Query { message: String },
    #[error("Ungültige Eingabe: {message}")]
    InvalidInput { message: String },
    #[error("Funktion nicht implementiert: {message}")]
    NotImplemented { message: String },
}

impl DbError {
    pub fn connection<E: fmt::Display>(err: E) -> Self {
        DbError::Connection {
            message: err.to_string(),
        }
    }

    pub fn query<E: fmt::Display>(err: E) -> Self {
        DbError::Query {
            message: err.to_string(),
        }
    }

    pub fn invalid_input<E: fmt::Display>(message: E) -> Self {
        DbError::InvalidInput {
            message: message.to_string(),
        }
    }
}

#[async_trait]
pub trait DbAdminPort: Send + Sync {
    async fn ping(&self) -> DbResult<()>;
    async fn simple_query(&self, statement: &str) -> DbResult<Vec<DbExecutionResult>>;
    async fn list_tables(&self) -> DbResult<Vec<DbTable>>;
    async fn describe_table(&self, table: &str) -> DbResult<DbTableSchema>;
}
