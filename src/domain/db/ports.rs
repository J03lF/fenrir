use async_trait::async_trait;

use super::{DbExecutionResult, DbResult, DbTable, DbTableSchema, DbValue};

#[async_trait]
pub trait DbAdminPort: Send + Sync {
    async fn ping(&self) -> DbResult<()>;
    async fn simple_query(&self, statement: &str) -> DbResult<Vec<DbExecutionResult>>;
    async fn list_tables(&self) -> DbResult<Vec<DbTable>>;
    async fn describe_table(&self, table: &str) -> DbResult<DbTableSchema>;
    async fn prepared_query(
        &self,
        statement: &str,
        params: &[DbValue],
    ) -> DbResult<Vec<DbExecutionResult>>;
    async fn prepared_execute(&self, statement: &str, params: &[DbValue]) -> DbResult<u64>;
}
