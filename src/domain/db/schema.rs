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
