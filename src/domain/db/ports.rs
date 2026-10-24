use async_trait::async_trait;

use super::{
    DbCallContext, DbExecutionResult, DbPoolStatus, DbResult, DbTable, DbTableSchema, DbValue,
};

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

    /// `simple_query` on behalf of `ctx` (pool, read-only, session effect).
    /// Adapters without pooling ignore the context.
    async fn simple_query_in(
        &self,
        ctx: &DbCallContext,
        statement: &str,
    ) -> DbResult<Vec<DbExecutionResult>> {
        let _ = ctx;
        self.simple_query(statement).await
    }

    /// `prepared_query` on behalf of `ctx`.
    async fn prepared_query_in(
        &self,
        ctx: &DbCallContext,
        statement: &str,
        params: &[DbValue],
    ) -> DbResult<Vec<DbExecutionResult>> {
        let _ = ctx;
        self.prepared_query(statement, params).await
    }

    /// `prepared_execute` on behalf of `ctx`.
    async fn prepared_execute_in(
        &self,
        ctx: &DbCallContext,
        statement: &str,
        params: &[DbValue],
    ) -> DbResult<u64> {
        let _ = ctx;
        self.prepared_execute(statement, params).await
    }

    /// Pool counters, when the adapter pools connections.
    fn pool_status(&self) -> Option<DbPoolStatus> {
        None
    }
}
