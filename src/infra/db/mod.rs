pub mod adapters;
pub mod migrations;

#[derive(Debug)]
pub enum DbEngine {
    Postgres,
    Mysql,
    Sqlite,
    Mongodb,
}

pub trait Database: Send + Sync {
    fn connect(&self) -> anyhow::Result<()>;
    fn ping(&self) -> anyhow::Result<()>;
}
