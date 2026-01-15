//! Intelligent SQL Completion Engine.
//!
//! This module provides context-aware SQL completion for the db shell.
//! It uses a tokenizer, context analyzer, and schema cache to provide
//! accurate suggestions based on the current position in a SQL query.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                        SQL Completion Engine                        │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │                                                                     │
//! │  ┌──────────────┐    ┌──────────────┐    ┌──────────────────────┐  │
//! │  │   Tokenizer  │───▶│   Context    │───▶│  Suggestion Engine   │  │
//! │  │  (Robust)    │    │   Analyzer   │    │  (Schema + Keywords) │  │
//! │  └──────────────┘    └──────────────┘    └──────────────────────┘  │
//! │                                                                     │
//! │  ┌──────────────────────────────────────────────────────────────┐  │
//! │  │                     Schema Cache                              │  │
//! │  │  - Tables: users, tickets, comments, ...                     │  │
//! │  │  - Columns: users.id, users.name, users.email, ...           │  │
//! │  └──────────────────────────────────────────────────────────────┘  │
//! │                                                                     │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use fenrir::cli::sql_completion::{SqlCompletionEngine, SchemaCache};
//!
//! // Create engine with schema
//! let mut engine = SqlCompletionEngine::new();
//! engine.set_tables(["users", "tickets"]);
//! engine.add_columns("users", vec!["id".into(), "name".into()]);
//!
//! // Get completions
//! let result = engine.complete("SELECT * FROM ", 14);
//! assert!(result.contains("users"));
//! assert!(result.contains("tickets"));
//! ```

mod tokenizer;
mod context;
mod schema;
mod engine;
pub mod keywords;

// Re-export main types
pub use tokenizer::{SqlTokenizer, Token, TokenKind, SqlKeywordKind};
pub use context::{SqlContext, StatementKind, ClauseKind, Expecting};
pub use schema::SchemaCache;
pub use engine::{SqlCompletionEngine, CompletionResult};

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::collections::HashMap;
    
    /// Create a test engine with realistic schema
    fn create_test_engine() -> SqlCompletionEngine {
        let mut columns = HashMap::new();
        columns.insert("users".to_string(), vec![
            "id".into(), "name".into(), "email".into(), "role".into(), "created_at".into()
        ]);
        columns.insert("tickets".to_string(), vec![
            "id".into(), "user_id".into(), "title".into(), "description".into(), 
            "status".into(), "priority".into(), "created_at".into()
        ]);
        columns.insert("comments".to_string(), vec![
            "id".into(), "ticket_id".into(), "user_id".into(), "content".into(), "created_at".into()
        ]);
        columns.insert("tags".to_string(), vec![
            "id".into(), "name".into(), "color".into()
        ]);
        
        SqlCompletionEngine::with_tables_and_columns(
            ["users", "tickets", "comments", "tags"],
            columns,
        )
    }
    
    fn c(engine: &SqlCompletionEngine, sql: &str) -> Vec<String> {
        engine.complete(sql, sql.len()).sorted_unique()
    }
    
    fn has(result: &[String], expected: &str) -> bool {
        result.iter().any(|s| s.eq_ignore_ascii_case(expected))
    }
    
    fn not_has(result: &[String], expected: &str) -> bool {
        !has(result, expected)
    }
    
    // ═══════════════════════════════════════════════════════════════
    // COMPREHENSIVE TEST SUITE
    // ═══════════════════════════════════════════════════════════════
    
    #[test]
    fn test_empty_suggests_statements() {
        let engine = create_test_engine();
        let result = c(&engine, "");
        assert!(has(&result, "SELECT"), "Missing SELECT");
        assert!(has(&result, "INSERT"), "Missing INSERT");
        assert!(has(&result, "UPDATE"), "Missing UPDATE");
        assert!(has(&result, "DELETE"), "Missing DELETE");
        assert!(not_has(&result, "FROM"), "FROM should not be suggested at start");
        assert!(not_has(&result, "WHERE"), "WHERE should not be suggested at start");
    }
    
    #[test]
    fn test_select_star_only_from() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * ");
        assert!(has(&result, "FROM"), "FROM should be suggested after SELECT *");
        assert!(not_has(&result, "*"), "* should not be suggested after SELECT *");
        assert!(not_has(&result, "DISTINCT"), "DISTINCT should not be suggested after SELECT *");
        assert!(not_has(&result, "WHERE"), "WHERE should not be suggested before FROM");
    }
    
    #[test]
    fn test_select_star_f_only_from() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * F");
        assert!(has(&result, "FROM"), "FROM should be suggested");
        // This was the original bug - suggesting FALSE after SELECT *
        assert!(result.len() == 1, "Only FROM should be suggested, got: {:?}", result);
    }
    
    #[test]
    fn test_insert_only_into() {
        let engine = create_test_engine();
        let result = c(&engine, "INSERT ");
        assert!(has(&result, "INTO"), "INTO should be suggested after INSERT");
        assert!(result.len() == 1, "Only INTO should be suggested after INSERT, got: {:?}", result);
    }
    
    #[test]
    fn test_update_table_only_set() {
        let engine = create_test_engine();
        let result = c(&engine, "UPDATE users ");
        assert!(has(&result, "SET"), "SET should be suggested after UPDATE table");
        assert!(not_has(&result, "WHERE"), "WHERE should not be suggested before SET");
        assert!(not_has(&result, "FROM"), "FROM should not be suggested for UPDATE");
    }
    
    #[test]
    fn test_delete_only_from() {
        let engine = create_test_engine();
        let result = c(&engine, "DELETE ");
        assert!(has(&result, "FROM"), "FROM should be suggested after DELETE");
        assert!(result.len() == 1, "Only FROM should be suggested after DELETE, got: {:?}", result);
    }
    
    #[test]
    fn test_from_suggests_tables() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM ");
        assert!(has(&result, "users"), "users should be suggested");
        assert!(has(&result, "tickets"), "tickets should be suggested");
        assert!(has(&result, "comments"), "comments should be suggested");
        assert!(has(&result, "tags"), "tags should be suggested");
        assert!(not_has(&result, "SELECT"), "SELECT should not be suggested after FROM");
        assert!(not_has(&result, "FROM"), "FROM should not be suggested after FROM");
    }
    
    #[test]
    fn test_where_suggests_columns() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM users WHERE ");
        assert!(has(&result, "id"), "id should be suggested in WHERE");
        assert!(has(&result, "name"), "name should be suggested in WHERE");
        assert!(has(&result, "email"), "email should be suggested in WHERE");
        assert!(not_has(&result, "ticket_id"), "ticket_id should not be suggested (not in users)");
    }
    
    #[test]
    fn test_where_column_suggests_operators() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM users WHERE id ");
        assert!(has(&result, "="), "= should be suggested");
        assert!(has(&result, "IN"), "IN should be suggested");
        assert!(has(&result, "LIKE"), "LIKE should be suggested");
        assert!(not_has(&result, "AND"), "AND should not be suggested before operator");
    }
    
    #[test]
    fn test_join_suggests_tables() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM users JOIN ");
        assert!(has(&result, "tickets"), "tickets should be suggested for JOIN");
        assert!(has(&result, "comments"), "comments should be suggested for JOIN");
    }
    
    #[test]
    fn test_join_table_suggests_on() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM users JOIN tickets ");
        assert!(has(&result, "ON"), "ON should be suggested after JOIN table");
    }
    
    #[test]
    fn test_alias_qualified_column() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM users u WHERE u.");
        assert!(has(&result, "id"), "u.id should be suggested");
        assert!(has(&result, "name"), "u.name should be suggested");
        assert!(has(&result, "email"), "u.email should be suggested");
        assert!(not_has(&result, "ticket_id"), "ticket_id should not be in users");
    }
    
    #[test]
    fn test_order_by_suggests_columns() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM users ORDER BY ");
        assert!(has(&result, "id"), "id should be suggested for ORDER BY");
        assert!(has(&result, "name"), "name should be suggested for ORDER BY");
        assert!(not_has(&result, "ORDER"), "ORDER should not be repeated");
    }
    
    #[test]
    fn test_order_by_column_suggests_modifiers() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM users ORDER BY name ");
        assert!(has(&result, "ASC"), "ASC should be suggested");
        assert!(has(&result, "DESC"), "DESC should be suggested");
        assert!(has(&result, "LIMIT"), "LIMIT should be suggested");
    }
    
    #[test]
    fn test_complex_query() {
        let engine = create_test_engine();
        
        // Step by step complex query
        let result = c(&engine, "SELECT ");
        assert!(has(&result, "*"));
        
        let result = c(&engine, "SELECT * ");
        assert!(has(&result, "FROM"));
        
        let result = c(&engine, "SELECT * FROM ");
        assert!(has(&result, "users"));
        
        let result = c(&engine, "SELECT * FROM users ");
        assert!(has(&result, "WHERE"));
        assert!(has(&result, "JOIN"));
        
        let result = c(&engine, "SELECT * FROM users JOIN ");
        assert!(has(&result, "tickets"));
        
        let result = c(&engine, "SELECT * FROM users u JOIN tickets t ON ");
        assert!(has(&result, "u.id"));
        assert!(has(&result, "t.user_id"));
    }
    
    #[test]
    fn test_semicolon_resets_context() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM users; ");
        assert!(has(&result, "SELECT"), "New statement should suggest SELECT");
        assert!(has(&result, "INSERT"), "New statement should suggest INSERT");
    }
    
    #[test]
    fn test_case_insensitive() {
        let engine = create_test_engine();
        
        let result = c(&engine, "select * from ");
        assert!(has(&result, "users"), "Lowercase keywords should work");
        
        let result = c(&engine, "SELECT * FROM u");
        assert!(has(&result, "users"), "Prefix matching should work");
    }
    
    #[test]
    fn test_set_suggests_columns() {
        let engine = create_test_engine();
        let result = c(&engine, "UPDATE users SET ");
        assert!(has(&result, "name"), "name should be suggested in SET");
        assert!(has(&result, "email"), "email should be suggested in SET");
    }
    
    #[test]
    fn test_insert_into_table_suggests_values() {
        let engine = create_test_engine();
        let result = c(&engine, "INSERT INTO users ");
        assert!(has(&result, "VALUES"), "VALUES should be suggested");
        assert!(has(&result, "("), "( should be suggested for column list");
    }
    
    #[test]
    fn test_group_by() {
        let engine = create_test_engine();
        let result = c(&engine, "SELECT * FROM users GROUP BY ");
        assert!(has(&result, "id"), "id should be suggested for GROUP BY");
        assert!(has(&result, "role"), "role should be suggested for GROUP BY");
    }
}

