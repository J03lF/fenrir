use crate::infra::db::Database;
use anyhow::Result;

pub struct PostgresAdapter;

impl PostgresAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PostgresAdapter {
    fn default() -> Self { Self::new() }
}

impl Database for PostgresAdapter {
    fn connect(&self) -> Result<()> {
        Ok(())
    }
    fn ping(&self) -> Result<()> {
        Ok(())
    }
}
