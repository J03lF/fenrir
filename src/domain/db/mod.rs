mod engine;
mod error;
mod ports;
mod result;
mod schema;
mod value;

pub use engine::DbEngine;
pub use error::DbError;
pub use ports::DbAdminPort;
pub use result::{DbExecutionResult, DbResultSet};
pub use schema::{DbColumn, DbTable, DbTableKind, DbTableSchema};
pub use value::DbValue;

pub type DbResult<T> = Result<T, DbError>;
