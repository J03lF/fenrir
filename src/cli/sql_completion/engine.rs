//! SQL Completion Engine.
//!
//! This is the main entry point for SQL completion. It combines:
//! - Tokenizer for parsing SQL input
//! - Context analyzer for understanding query structure
//! - Schema cache for table/column information
//! - Suggestion generation based on context

use super::context::{Expecting, SqlContext};
use super::keywords;
use super::schema::SchemaCache;
use super::tokenizer::{SqlTokenizer, Token, TokenKind};
use std::collections::{HashMap, HashSet};

/// The SQL Completion Engine
pub struct SqlCompletionEngine {
    /// Schema cache with tables and columns
    schema: SchemaCache,
}

impl SqlCompletionEngine {
    /// Create a new completion engine with empty schema
    pub fn new() -> Self {
        Self {
            schema: SchemaCache::new(),
        }
    }

    /// Create a new completion engine with the given schema
    pub fn with_schema(schema: SchemaCache) -> Self {
        Self { schema }
    }

    /// Create a completion engine for testing with tables and columns
    pub fn with_tables_and_columns(
        tables: impl IntoIterator<Item = impl Into<String>>,
        columns: HashMap<String, Vec<String>>,
    ) -> Self {
        Self {
            schema: SchemaCache::with_schema(tables, columns),
        }
    }

    /// Update the schema cache
    pub fn update_schema(
        &mut self,
        tables: HashSet<String>,
        columns: HashMap<String, Vec<String>>,
    ) {
        self.schema.update(tables, columns);
    }

    /// Set table names only
    pub fn set_tables(&mut self, tables: impl IntoIterator<Item = impl Into<String>>) {
        for table in tables {
            self.schema.add_table(table);
        }
    }

    /// Add columns for a table
    pub fn add_columns(&mut self, table: impl Into<String>, columns: Vec<String>) {
        self.schema.add_columns(table, columns);
    }

    /// Get completion suggestions for the given SQL at the cursor position
    pub fn complete(&self, sql: &str, cursor_pos: usize) -> CompletionResult {
        let pos = cursor_pos.min(sql.len());
        let before_cursor = &sql[..pos];

        // Tokenize and analyze
        let tokens = SqlTokenizer::tokenize(before_cursor);
        let context = SqlContext::from_tokens(&tokens);

        // Find prefix for filtering (the partial word being typed)
        let (prefix_start, prefix) = self.find_prefix(before_cursor, &tokens);
        let prefix_upper = prefix.to_uppercase();

        // Generate suggestions based on context
        let suggestions = self.generate_suggestions(&context, &prefix_upper, &tokens);

        CompletionResult {
            start: prefix_start,
            prefix: prefix.to_string(),
            suggestions,
            context,
        }
    }

    /// Find the prefix (partial word) the user is typing
    fn find_prefix<'a>(&self, before_cursor: &'a str, tokens: &[Token]) -> (usize, &'a str) {
        // Find start of current token
        let start = before_cursor
            .rfind(|c: char| {
                c.is_whitespace() || c == '(' || c == ')' || c == ',' || c == '=' || c == ';'
            })
            .map(|idx| idx + 1)
            .unwrap_or(0);

        let prefix = &before_cursor[start..];

        // Special case: if prefix ends with '.', we're completing after a dot
        if prefix.ends_with('.') {
            // Return empty prefix but include the qualifier
            return (before_cursor.len(), "");
        }

        if let Some(dot_pos) = prefix.rfind('.') {
            // Prefix contains a dot: "table.col" -> complete "col" part
            return (start + dot_pos + 1, &prefix[dot_pos + 1..]);
        }

        // Check if the last token is a punctuation mark that ends completion
        // If cursor is right after *, (, ), etc. we want to suggest what comes next
        if let Some(last_token) = tokens.last() {
            if last_token.end == before_cursor.len() {
                match last_token.kind {
                    TokenKind::Star
                    | TokenKind::Comma
                    | TokenKind::OpenParen
                    | TokenKind::CloseParen
                    | TokenKind::Semicolon => {
                        // These are single-char complete tokens
                        // Prefix should be empty to suggest what comes next
                        return (before_cursor.len(), "");
                    }
                    _ => {
                        // For identifiers and keywords, the prefix is the token text
                        // This allows partial matching (e.g., "u" matches "users")
                    }
                }
            }
        }

        (start, prefix)
    }

    /// Generate suggestions based on context
    fn generate_suggestions(
        &self,
        context: &SqlContext,
        prefix: &str,
        tokens: &[Token],
    ) -> Vec<String> {
        match context.expecting {
            // Statement start
            Expecting::Statement => self.filter_keywords(keywords::STATEMENT_KEYWORDS, prefix),

            // SELECT clause
            Expecting::ColumnOrStar => {
                let mut sugg = Vec::new();
                sugg.push("*".to_string());
                sugg.extend(self.filter_keywords(keywords::SELECT_MODIFIERS, prefix));
                sugg.extend(self.filter_keywords(keywords::AGGREGATE_FUNCTIONS, prefix));
                // Add columns if we have tables in scope
                if !context.tables.is_empty() {
                    sugg.extend(self.schema.columns_with_prefix(prefix, &context.tables));
                }
                sugg
            }

            // FROM keyword required (after SELECT *)
            Expecting::FromKeyword => self.filter_keywords(&["FROM"], prefix),

            // INTO keyword required (after INSERT)
            Expecting::IntoKeyword => self.filter_keywords(&["INTO"], prefix),

            // Table name expected
            Expecting::TableName => self.schema.tables_with_prefix(prefix),

            // Column name expected
            Expecting::ColumnName => self.columns_for_context(context, prefix),

            // Column or expression
            Expecting::ColumnOrExpression => {
                let mut sugg = self.columns_for_context(context, prefix);
                // Add functions for expressions
                sugg.extend(self.filter_keywords(keywords::AGGREGATE_FUNCTIONS, prefix));
                sugg.extend(self.filter_keywords(keywords::SCALAR_FUNCTIONS, prefix));
                sugg
            }

            // After table.
            Expecting::QualifiedColumn => {
                self.qualified_columns_for_context(context, tokens, prefix)
            }

            // Comma or FROM (after column in SELECT)
            Expecting::CommaOrFrom => {
                // If prefix is non-empty, user might still be typing column name
                if !prefix.is_empty() {
                    let cols = self.columns_for_context(context, prefix);
                    if !cols.is_empty() {
                        return cols; // Still typing column name
                    }
                }
                let mut sugg = vec![];
                if prefix.is_empty() || ",".starts_with(prefix) {
                    sugg.push(",".to_string());
                }
                sugg.extend(self.filter_keywords(&["AS", "FROM"], prefix));
                sugg
            }

            // Clause or JOIN (after FROM table)
            Expecting::ClauseOrJoin => {
                let mut sugg = Vec::new();
                // If prefix is non-empty, user might still be typing table name
                if !prefix.is_empty() {
                    let tables = self.schema.tables_with_prefix(prefix);
                    if !tables.is_empty() {
                        return tables; // Still typing table name
                    }
                }
                sugg.extend(self.filter_keywords(&["WHERE"], prefix));
                sugg.extend(self.filter_keywords(keywords::JOIN_TYPES, prefix));
                sugg.extend(self.filter_keywords(&["ORDER", "GROUP", "HAVING", "LIMIT"], prefix));
                sugg.extend(self.filter_keywords(&["UNION", "INTERSECT", "EXCEPT"], prefix));
                sugg
            }

            // ON or other clause (after JOIN table)
            Expecting::OnOrClause => {
                // If prefix is non-empty, user might still be typing table name
                if !prefix.is_empty() {
                    let tables = self.schema.tables_with_prefix(prefix);
                    if !tables.is_empty() {
                        return tables; // Still typing table name
                    }
                }
                self.filter_keywords(&["ON", "WHERE", "ORDER", "GROUP"], prefix)
            }

            // Comparison operator
            Expecting::Operator => {
                // If prefix is non-empty, user might still be typing column name
                if !prefix.is_empty() {
                    let cols = self.columns_for_context(context, prefix);
                    if !cols.is_empty() {
                        return cols; // Still typing column name
                    }
                }
                self.filter_keywords(keywords::COMPARISON_OPERATORS, prefix)
            }

            // Value - no suggestions (user types literal)
            Expecting::Value => {
                // Could offer NULL, TRUE, FALSE
                self.filter_keywords(&["NULL", "TRUE", "FALSE"], prefix)
            }

            // Alias - no suggestions (user types alias name)
            Expecting::Alias => {
                vec![]
            }

            // ASC/DESC or comma (after ORDER BY column)
            Expecting::OrderModifierOrComma => {
                let mut sugg = Vec::new();
                sugg.extend(self.filter_keywords(keywords::ORDER_MODIFIERS, prefix));
                if prefix.is_empty() || ",".starts_with(prefix) {
                    sugg.push(",".to_string());
                }
                sugg.extend(self.filter_keywords(&["LIMIT", "OFFSET"], prefix));
                sugg
            }

            // Aggregate or column (for HAVING)
            Expecting::AggregateOrColumn => {
                let mut sugg = self.filter_keywords(keywords::AGGREGATE_FUNCTIONS, prefix);
                sugg.extend(self.columns_for_context(context, prefix));
                sugg
            }

            // VALUES or column list (after INSERT INTO table)
            Expecting::ValuesOrColumns => {
                let mut sugg = Vec::new();
                sugg.extend(self.filter_keywords(&["VALUES"], prefix));
                if prefix.is_empty() || "(".starts_with(prefix) {
                    sugg.push("(".to_string());
                }
                sugg.extend(self.filter_keywords(&["SELECT"], prefix));
                sugg
            }

            // Value list - no suggestions
            Expecting::ValueList => {
                vec![]
            }

            // Equals sign (for SET)
            Expecting::Equals => {
                if prefix.is_empty() || "=".starts_with(prefix) {
                    vec!["=".to_string()]
                } else {
                    vec![]
                }
            }

            // Number (for LIMIT/OFFSET) - no suggestions
            Expecting::Number => {
                vec![]
            }

            // DDL object type
            Expecting::DdlObject => self.filter_keywords(keywords::CREATE_OBJECTS, prefix),

            // SET keyword required (after UPDATE table)
            Expecting::SetKeyword => {
                // If prefix is non-empty, user might still be typing table name
                if !prefix.is_empty() {
                    let tables = self.schema.tables_with_prefix(prefix);
                    if !tables.is_empty() {
                        return tables; // Still typing table name
                    }
                }
                self.filter_keywords(&["SET"], prefix)
            }

            // Join condition
            Expecting::JoinCondition => {
                // Suggest qualified columns from all tables in context
                let mut sugg = Vec::new();
                for table in &context.tables {
                    // Use alias if available
                    let qualifier = context
                        .aliases
                        .iter()
                        .find(|(_, t)| *t == table)
                        .map(|(a, _)| a.as_str())
                        .unwrap_or(table.as_str());

                    if let Some(cols) = self.schema.columns_for(table) {
                        for col in cols {
                            let qualified = format!("{}.{}", qualifier, col);
                            if prefix.is_empty()
                                || qualified.to_uppercase().starts_with(&prefix.to_uppercase())
                            {
                                sugg.push(qualified);
                            }
                        }
                    }
                }
                sugg
            }
        }
    }

    /// Get columns appropriate for the current context
    fn columns_for_context(&self, context: &SqlContext, prefix: &str) -> Vec<String> {
        if context.tables.is_empty() {
            // No tables in scope - return all known columns
            self.schema.columns_with_prefix(prefix, &[])
        } else {
            // Return columns from tables in scope
            self.schema.columns_with_prefix(prefix, &context.tables)
        }
    }

    /// Get qualified columns after a dot (table.column)
    fn qualified_columns_for_context(
        &self,
        context: &SqlContext,
        tokens: &[Token],
        prefix: &str,
    ) -> Vec<String> {
        // Find the qualifier (table or alias) before the dot
        let qualifier = tokens
            .iter()
            .rev()
            .skip_while(|t| t.kind == TokenKind::Dot)
            .find(|t| t.is_identifier())
            .map(|t| t.text.as_str());

        if let Some(q) = qualifier {
            // Resolve alias if needed
            let table_name = context.resolve_alias(q).map(|s| s.as_str()).unwrap_or(q);

            if let Some(cols) = self.schema.columns_for(table_name) {
                let prefix_lower = prefix.to_lowercase();
                return cols
                    .iter()
                    .filter(|c| prefix.is_empty() || c.to_lowercase().starts_with(&prefix_lower))
                    .cloned()
                    .collect();
            }
        }

        vec![]
    }

    /// Filter keywords by prefix
    fn filter_keywords(&self, keywords: &[&str], prefix: &str) -> Vec<String> {
        if prefix.is_empty() {
            return keywords.iter().map(|s| s.to_string()).collect();
        }
        let prefix_upper = prefix.to_uppercase();
        keywords
            .iter()
            .filter(|kw| kw.to_uppercase().starts_with(&prefix_upper))
            .map(|s| s.to_string())
            .collect()
    }
}

impl Default for SqlCompletionEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a completion request
#[derive(Debug)]
pub struct CompletionResult {
    /// Position where the prefix starts (for replacement)
    pub start: usize,
    /// The prefix being completed
    pub prefix: String,
    /// List of suggestions
    pub suggestions: Vec<String>,
    /// The analyzed context (for debugging/testing)
    pub context: SqlContext,
}

impl CompletionResult {
    /// Get suggestions sorted and deduplicated
    pub fn sorted_unique(&self) -> Vec<String> {
        let mut sugg = self.suggestions.clone();
        sugg.sort();
        sugg.dedup();
        sugg
    }

    /// Check if a specific suggestion is present
    pub fn contains(&self, suggestion: &str) -> bool {
        self.suggestions
            .iter()
            .any(|s| s.eq_ignore_ascii_case(suggestion))
    }

    /// Check that a suggestion is NOT present
    pub fn not_contains(&self, suggestion: &str) -> bool {
        !self.contains(suggestion)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine() -> SqlCompletionEngine {
        let mut columns = HashMap::new();
        columns.insert(
            "users".to_string(),
            vec!["id".into(), "name".into(), "email".into()],
        );
        columns.insert(
            "tickets".to_string(),
            vec![
                "id".into(),
                "user_id".into(),
                "title".into(),
                "status".into(),
            ],
        );
        columns.insert(
            "comments".to_string(),
            vec!["id".into(), "ticket_id".into(), "content".into()],
        );

        SqlCompletionEngine::with_tables_and_columns(["users", "tickets", "comments"], columns)
    }

    fn complete(engine: &SqlCompletionEngine, sql: &str) -> CompletionResult {
        engine.complete(sql, sql.len())
    }

    // ═══════════════════════════════════════════════════════════════
    // EMPTY INPUT
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_empty_input() {
        let engine = test_engine();
        let result = complete(&engine, "");
        assert!(result.contains("SELECT"));
        assert!(result.contains("INSERT"));
        assert!(result.contains("UPDATE"));
        assert!(result.contains("DELETE"));
        assert!(result.not_contains("FROM"));
        assert!(result.not_contains("WHERE"));
    }

    #[test]
    fn test_partial_keyword() {
        let engine = test_engine();
        let result = complete(&engine, "S");
        assert!(result.contains("SELECT"));
        assert!(result.not_contains("INSERT"));
    }

    // ═══════════════════════════════════════════════════════════════
    // SELECT STATEMENT
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_after_select() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT ");
        assert!(result.contains("*"));
        assert!(result.contains("DISTINCT"));
        assert!(result.not_contains("FROM"));
        assert!(result.not_contains("SELECT"));
    }

    #[test]
    fn test_after_select_star() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT *");
        // After SELECT * the ONLY valid option is FROM
        assert!(result.contains("FROM"));
        assert!(result.not_contains("*"));
        assert!(result.not_contains("WHERE"));
        assert!(result.not_contains("DISTINCT"));
    }

    #[test]
    fn test_after_select_star_space() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * ");
        assert!(result.contains("FROM"));
        assert!(result.not_contains("*"));
        assert!(result.not_contains("WHERE"));
    }

    #[test]
    fn test_after_select_star_f() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * F");
        assert!(result.contains("FROM"));
        assert!(result.not_contains("FALSE")); // This was the bug!
    }

    #[test]
    fn test_after_select_column() {
        let engine = test_engine();
        // With space after column, expect comma/FROM
        let result = complete(&engine, "SELECT id ");
        assert!(result.contains(","));
        assert!(result.contains("FROM"));
        assert!(result.contains("AS"));
    }

    #[test]
    fn test_select_column_partial() {
        let engine = test_engine();
        // Without space, user might still be typing - suggest matching columns
        let result = complete(&engine, "SELECT id");
        // "id" is a column, so it matches
        assert!(result.contains("id"));
    }

    #[test]
    fn test_after_select_comma() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT id, ");
        assert!(result.contains("*"));
        assert!(result.not_contains("FROM"));
    }

    #[test]
    fn test_after_from() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM ");
        assert!(result.contains("users"));
        assert!(result.contains("tickets"));
        assert!(result.contains("comments"));
        assert!(result.not_contains("SELECT"));
        assert!(result.not_contains("FROM"));
    }

    #[test]
    fn test_after_from_table() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users ");
        assert!(result.contains("WHERE"));
        assert!(result.contains("JOIN"));
        assert!(result.contains("LEFT"));
        assert!(result.contains("ORDER"));
        assert!(result.not_contains("FROM"));
        assert!(result.not_contains("*"));
    }

    #[test]
    fn test_after_where() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users WHERE ");
        // Should suggest columns from users table
        assert!(result.contains("id"));
        assert!(result.contains("name"));
        assert!(result.contains("email"));
        assert!(result.not_contains("FROM"));
        assert!(result.not_contains("WHERE"));
    }

    #[test]
    fn test_after_where_column() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users WHERE id ");
        // Should suggest operators
        assert!(result.contains("="));
        assert!(result.contains("IN"));
        assert!(result.contains("LIKE"));
        assert!(result.not_contains("AND"));
    }

    #[test]
    fn test_after_where_operator() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users WHERE id = ");
        // Value expected - limited suggestions
        assert!(result.contains("NULL"));
        assert!(result.not_contains("="));
    }

    // ═══════════════════════════════════════════════════════════════
    // INSERT STATEMENT
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_after_insert() {
        let engine = test_engine();
        let result = complete(&engine, "INSERT ");
        assert!(result.contains("INTO"));
        assert!(result.not_contains("SELECT"));
        assert!(result.not_contains("VALUES"));
    }

    #[test]
    fn test_after_insert_into() {
        let engine = test_engine();
        let result = complete(&engine, "INSERT INTO ");
        assert!(result.contains("users"));
        assert!(result.contains("tickets"));
        assert!(result.not_contains("INTO"));
    }

    #[test]
    fn test_after_insert_into_table() {
        let engine = test_engine();
        let result = complete(&engine, "INSERT INTO users ");
        assert!(result.contains("VALUES"));
        assert!(result.contains("("));
        assert!(result.not_contains("INTO"));
    }

    // ═══════════════════════════════════════════════════════════════
    // UPDATE STATEMENT
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_after_update() {
        let engine = test_engine();
        let result = complete(&engine, "UPDATE ");
        assert!(result.contains("users"));
        assert!(result.contains("tickets"));
        assert!(result.not_contains("SET"));
    }

    #[test]
    fn test_after_update_table() {
        let engine = test_engine();
        let result = complete(&engine, "UPDATE users ");
        // After UPDATE table, only SET is valid
        assert!(result.contains("SET"));
        assert!(result.not_contains("WHERE"));
        assert!(result.not_contains("FROM"));
    }

    #[test]
    fn test_after_set() {
        let engine = test_engine();
        let result = complete(&engine, "UPDATE users SET ");
        // Should suggest columns
        assert!(result.contains("id"));
        assert!(result.contains("name"));
        assert!(result.not_contains("SET"));
    }

    // ═══════════════════════════════════════════════════════════════
    // DELETE STATEMENT
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_after_delete() {
        let engine = test_engine();
        let result = complete(&engine, "DELETE ");
        assert!(result.contains("FROM"));
        assert!(result.not_contains("SELECT"));
        assert!(result.not_contains("WHERE"));
    }

    #[test]
    fn test_after_delete_from() {
        let engine = test_engine();
        let result = complete(&engine, "DELETE FROM ");
        assert!(result.contains("users"));
        assert!(result.contains("tickets"));
        assert!(result.not_contains("FROM"));
    }

    // ═══════════════════════════════════════════════════════════════
    // JOIN
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_after_join() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users JOIN ");
        assert!(result.contains("tickets"));
        assert!(result.contains("comments"));
    }

    #[test]
    fn test_after_join_table() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users JOIN tickets ");
        assert!(result.contains("ON"));
    }

    // ═══════════════════════════════════════════════════════════════
    // ALIASES
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_qualified_column() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users u WHERE u.");
        // Should suggest columns from users
        assert!(result.contains("id"));
        assert!(result.contains("name"));
        assert!(result.contains("email"));
        assert!(result.not_contains("u"));
    }

    #[test]
    fn test_alias_in_join() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users u JOIN tickets t ON u.");
        assert!(result.contains("id"));
        assert!(result.contains("name"));
    }

    // ═══════════════════════════════════════════════════════════════
    // ORDER BY / GROUP BY
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_after_order_by() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users ORDER BY ");
        assert!(result.contains("id"));
        assert!(result.contains("name"));
        assert!(result.not_contains("ORDER"));
    }

    #[test]
    fn test_after_order_by_column() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users ORDER BY name ");
        assert!(result.contains("ASC"));
        assert!(result.contains("DESC"));
        assert!(result.contains(","));
        assert!(result.contains("LIMIT"));
    }

    // ═══════════════════════════════════════════════════════════════
    // EDGE CASES
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_lowercase() {
        let engine = test_engine();
        let result = complete(&engine, "select * from ");
        assert!(result.contains("users"));
    }

    #[test]
    fn test_mixed_case() {
        let engine = test_engine();
        let result = complete(&engine, "SeLeCt * FrOm ");
        assert!(result.contains("users"));
    }

    #[test]
    fn test_multiple_spaces() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT  *  FROM  ");
        assert!(result.contains("users"));
    }

    #[test]
    fn test_after_semicolon() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users; ");
        assert!(result.contains("SELECT"));
        assert!(result.contains("INSERT"));
    }

    #[test]
    fn test_partial_table_name() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM u");
        assert!(result.contains("users"));
        assert!(result.not_contains("tickets"));
    }

    #[test]
    fn test_partial_keyword_where() {
        let engine = test_engine();
        let result = complete(&engine, "SELECT * FROM users WH");
        assert!(result.contains("WHERE"));
    }
}
