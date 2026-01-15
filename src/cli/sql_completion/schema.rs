//! Schema cache for SQL completion.
//!
//! This module caches database schema information (tables, columns)
//! for fast completion lookups.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Default cache TTL (5 minutes)
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(300);

/// Schema cache holding database metadata for completion
#[derive(Debug, Clone)]
pub struct SchemaCache {
    /// All table names in the database
    tables: HashSet<String>,
    /// Columns for each table: table_name -> Vec<column_name>
    columns: HashMap<String, Vec<String>>,
    /// When the cache was last refreshed
    last_refresh: Option<Instant>,
    /// Cache TTL
    ttl: Duration,
}

impl Default for SchemaCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaCache {
    /// Create a new empty schema cache
    pub fn new() -> Self {
        Self {
            tables: HashSet::new(),
            columns: HashMap::new(),
            last_refresh: None,
            ttl: DEFAULT_CACHE_TTL,
        }
    }
    
    /// Create a schema cache with initial data
    pub fn with_tables(tables: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut cache = Self::new();
        for table in tables {
            cache.tables.insert(table.into());
        }
        cache.last_refresh = Some(Instant::now());
        cache
    }
    
    /// Create a schema cache with tables and columns
    pub fn with_schema(
        tables: impl IntoIterator<Item = impl Into<String>>,
        columns: HashMap<String, Vec<String>>,
    ) -> Self {
        let mut cache = Self::new();
        for table in tables {
            cache.tables.insert(table.into());
        }
        cache.columns = columns;
        cache.last_refresh = Some(Instant::now());
        cache
    }
    
    /// Add a table to the cache
    pub fn add_table(&mut self, name: impl Into<String>) {
        self.tables.insert(name.into());
    }
    
    /// Add columns for a table
    pub fn add_columns(&mut self, table: impl Into<String>, cols: Vec<String>) {
        let table_name = table.into();
        self.tables.insert(table_name.clone());
        self.columns.insert(table_name, cols);
    }
    
    /// Update the cache with new schema data
    pub fn update(&mut self, tables: HashSet<String>, columns: HashMap<String, Vec<String>>) {
        self.tables = tables;
        self.columns = columns;
        self.last_refresh = Some(Instant::now());
    }
    
    /// Clear the cache
    pub fn clear(&mut self) {
        self.tables.clear();
        self.columns.clear();
        self.last_refresh = None;
    }
    
    /// Check if the cache is stale
    pub fn is_stale(&self) -> bool {
        match self.last_refresh {
            Some(time) => time.elapsed() > self.ttl,
            None => true,
        }
    }
    
    /// Check if the cache has any data
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
    
    /// Get all table names
    pub fn tables(&self) -> &HashSet<String> {
        &self.tables
    }
    
    /// Get tables as a sorted vector
    pub fn tables_sorted(&self) -> Vec<String> {
        let mut tables: Vec<_> = self.tables.iter().cloned().collect();
        tables.sort();
        tables
    }
    
    /// Get columns for a specific table
    pub fn columns_for(&self, table: &str) -> Option<&Vec<String>> {
        // Try exact match first
        if let Some(cols) = self.columns.get(table) {
            return Some(cols);
        }
        // Try case-insensitive
        let lower = table.to_lowercase();
        for (t, cols) in &self.columns {
            if t.to_lowercase() == lower {
                return Some(cols);
            }
        }
        None
    }
    
    /// Get columns for multiple tables (for JOINs)
    pub fn columns_for_tables(&self, tables: &[String]) -> Vec<String> {
        let mut all_cols = Vec::new();
        for table in tables {
            if let Some(cols) = self.columns_for(table) {
                all_cols.extend(cols.iter().cloned());
            }
        }
        all_cols.sort();
        all_cols.dedup();
        all_cols
    }
    
    /// Get qualified column names for a table (table.column)
    pub fn qualified_columns_for(&self, table_or_alias: &str, actual_table: Option<&str>) -> Vec<String> {
        let table_name = actual_table.unwrap_or(table_or_alias);
        if let Some(cols) = self.columns_for(table_name) {
            cols.iter()
                .map(|c| format!("{}.{}", table_or_alias, c))
                .collect()
        } else {
            Vec::new()
        }
    }
    
    /// Get all columns from all tables (for WHERE without qualification)
    pub fn all_columns(&self) -> Vec<String> {
        let mut all_cols: Vec<String> = self.columns
            .values()
            .flat_map(|cols| cols.iter().cloned())
            .collect();
        all_cols.sort();
        all_cols.dedup();
        all_cols
    }
    
    /// Check if a name is a known table
    pub fn is_table(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        self.tables.iter().any(|t| t.to_lowercase() == lower)
    }
    
    /// Check if a name is a known column in any table
    pub fn is_column(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        self.columns.values()
            .any(|cols| cols.iter().any(|c| c.to_lowercase() == lower))
    }
    
    /// Filter tables by prefix
    pub fn tables_with_prefix(&self, prefix: &str) -> Vec<String> {
        if prefix.is_empty() {
            return self.tables_sorted();
        }
        let lower_prefix = prefix.to_lowercase();
        let mut matches: Vec<_> = self.tables
            .iter()
            .filter(|t| t.to_lowercase().starts_with(&lower_prefix))
            .cloned()
            .collect();
        matches.sort();
        matches
    }
    
    /// Filter columns by prefix
    pub fn columns_with_prefix(&self, prefix: &str, tables: &[String]) -> Vec<String> {
        let lower_prefix = prefix.to_lowercase();
        let cols = if tables.is_empty() {
            self.all_columns()
        } else {
            self.columns_for_tables(tables)
        };
        
        if prefix.is_empty() {
            return cols;
        }
        
        cols.into_iter()
            .filter(|c| c.to_lowercase().starts_with(&lower_prefix))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn test_schema() -> SchemaCache {
        let mut cache = SchemaCache::new();
        cache.add_table("users");
        cache.add_table("tickets");
        cache.add_table("comments");
        cache.add_columns("users", vec!["id".into(), "name".into(), "email".into()]);
        cache.add_columns("tickets", vec!["id".into(), "user_id".into(), "title".into(), "status".into()]);
        cache.add_columns("comments", vec!["id".into(), "ticket_id".into(), "content".into()]);
        cache
    }
    
    #[test]
    fn test_tables() {
        let cache = test_schema();
        assert!(cache.is_table("users"));
        assert!(cache.is_table("Users"));  // Case insensitive
        assert!(!cache.is_table("nonexistent"));
    }
    
    #[test]
    fn test_columns() {
        let cache = test_schema();
        let cols = cache.columns_for("users").unwrap();
        assert!(cols.contains(&"id".to_string()));
        assert!(cols.contains(&"name".to_string()));
        assert!(cols.contains(&"email".to_string()));
    }
    
    #[test]
    fn test_columns_case_insensitive() {
        let cache = test_schema();
        assert!(cache.columns_for("Users").is_some());
        assert!(cache.columns_for("USERS").is_some());
    }
    
    #[test]
    fn test_all_columns() {
        let cache = test_schema();
        let all = cache.all_columns();
        assert!(all.contains(&"id".to_string()));
        assert!(all.contains(&"name".to_string()));
        assert!(all.contains(&"title".to_string()));
        assert!(all.contains(&"content".to_string()));
    }
    
    #[test]
    fn test_tables_with_prefix() {
        let cache = test_schema();
        let matches = cache.tables_with_prefix("u");
        assert_eq!(matches, vec!["users"]);
        
        let matches = cache.tables_with_prefix("t");
        assert_eq!(matches, vec!["tickets"]);
        
        let matches = cache.tables_with_prefix("");
        assert_eq!(matches.len(), 3);
    }
    
    #[test]
    fn test_columns_for_tables() {
        let cache = test_schema();
        let cols = cache.columns_for_tables(&["users".into(), "tickets".into()]);
        assert!(cols.contains(&"name".to_string()));  // From users
        assert!(cols.contains(&"title".to_string())); // From tickets
    }
    
    #[test]
    fn test_qualified_columns() {
        let cache = test_schema();
        let qualified = cache.qualified_columns_for("u", Some("users"));
        assert!(qualified.contains(&"u.id".to_string()));
        assert!(qualified.contains(&"u.name".to_string()));
    }
    
    #[test]
    fn test_cache_stale() {
        let mut cache = SchemaCache::new();
        assert!(cache.is_stale());  // Empty cache is stale
        
        cache.add_table("test");
        cache.last_refresh = Some(Instant::now());
        assert!(!cache.is_stale());
        
        cache.ttl = Duration::from_millis(1);
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.is_stale());
    }
    
    #[test]
    fn test_with_tables() {
        let cache = SchemaCache::with_tables(["users", "tickets"]);
        assert!(cache.is_table("users"));
        assert!(cache.is_table("tickets"));
        assert!(!cache.is_stale());
    }
    
    #[test]
    fn test_columns_with_prefix() {
        let cache = test_schema();
        let matches = cache.columns_with_prefix("id", &[]);
        assert_eq!(matches, vec!["id"]);
        
        let matches = cache.columns_with_prefix("ti", &["tickets".into()]);
        assert_eq!(matches, vec!["title"]);
    }
    
    #[test]
    fn test_clear() {
        let mut cache = test_schema();
        assert!(!cache.is_empty());
        cache.clear();
        assert!(cache.is_empty());
    }
}

