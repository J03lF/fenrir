use std::fmt;
use std::str::FromStr;

use crate::utils::messages::domain::db as db_messages;

use super::DbError;

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
                message: db_messages::engine::unknown(other),
            }),
        }
    }
}
