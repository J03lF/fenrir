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
