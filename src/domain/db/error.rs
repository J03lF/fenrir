use std::fmt;

use crate::utils::messages::domain::db as db_messages;

use super::engine::DbEngine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbError {
    EngineNotConfigured { engine: DbEngine },
    Connection { message: String },
    Query { message: String },
    InvalidInput { message: String },
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

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::EngineNotConfigured { engine } => {
                f.write_str(&db_messages::engine::not_configured(engine))
            }
            DbError::Connection { message } => {
                f.write_str(&db_messages::errors::connection(message))
            }
            DbError::Query { message } => f.write_str(&db_messages::errors::query(message)),
            DbError::InvalidInput { message } => {
                f.write_str(&db_messages::errors::invalid_input(message))
            }
            DbError::NotImplemented { message } => {
                f.write_str(&db_messages::errors::not_implemented(message))
            }
        }
    }
}

impl std::error::Error for DbError {}
