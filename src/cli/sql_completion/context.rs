//! SQL Context tracking for intelligent completion.
//!
//! This module tracks the SQL structure to understand where in a query
//! the cursor is positioned. It maintains:
//! - Current statement type (SELECT, INSERT, UPDATE, DELETE)
//! - Current clause (FROM, WHERE, ORDER BY, etc.)
//! - Parentheses depth for subqueries
//! - Table aliases for qualified completions
//! - Tables in current FROM clause

use super::tokenizer::{SqlKeywordKind, Token, TokenKind};
use std::collections::HashMap;

/// The current SQL context at cursor position
#[derive(Debug, Clone)]
pub struct SqlContext {
    /// The type of statement being written
    pub statement: Option<StatementKind>,
    /// The current clause within the statement
    pub clause: ClauseKind,
    /// What we expect next based on current position
    pub expecting: Expecting,
    /// Tables currently in scope (from FROM clause)
    pub tables: Vec<String>,
    /// Alias to table mapping (u -> users)
    pub aliases: HashMap<String, String>,
    /// Current parentheses depth
    pub paren_depth: i32,
    /// Whether we're in a subquery
    pub in_subquery: bool,
    /// The last significant token before cursor
    pub last_token: Option<Token>,
    /// The token before last_token (for two-token lookback)
    pub prev_token: Option<Token>,
}

impl Default for SqlContext {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlContext {
    pub fn new() -> Self {
        Self {
            statement: None,
            clause: ClauseKind::None,
            expecting: Expecting::Statement,
            tables: Vec::new(),
            aliases: HashMap::new(),
            paren_depth: 0,
            in_subquery: false,
            last_token: None,
            prev_token: None,
        }
    }

    /// Analyze tokens and build context
    pub fn from_tokens(tokens: &[Token]) -> Self {
        let mut ctx = Self::new();
        ctx.analyze(tokens);
        ctx
    }

    /// Analyze the token sequence to determine context
    pub fn analyze(&mut self, tokens: &[Token]) {
        if tokens.is_empty() {
            self.expecting = Expecting::Statement;
            return;
        }

        let mut i = 0;
        while i < tokens.len() {
            let token = &tokens[i];

            // Update lookback tokens
            self.prev_token = self.last_token.clone();
            self.last_token = Some(token.clone());

            // Handle parentheses
            match token.kind {
                TokenKind::OpenParen => {
                    self.paren_depth += 1;
                    // Check if this starts a subquery
                    if i + 1 < tokens.len() && tokens[i + 1].is_keyword("SELECT") {
                        self.in_subquery = true;
                    }
                }
                TokenKind::CloseParen => {
                    self.paren_depth = (self.paren_depth - 1).max(0);
                    if self.paren_depth == 0 {
                        self.in_subquery = false;
                    }
                }
                _ => {}
            }

            // Handle semicolon - reset context for new statement
            if token.kind == TokenKind::Semicolon {
                self.reset_for_new_statement();
                i += 1;
                continue;
            }

            // Process based on current state
            self.process_token(token, tokens, i);
            i += 1;
        }

        // Determine final expectation based on context
        self.determine_expectation();
    }

    fn reset_for_new_statement(&mut self) {
        self.statement = None;
        self.clause = ClauseKind::None;
        self.expecting = Expecting::Statement;
        self.tables.clear();
        self.aliases.clear();
        self.paren_depth = 0;
        self.in_subquery = false;
        self.last_token = None;
        self.prev_token = None;
    }

    fn process_token(&mut self, token: &Token, tokens: &[Token], idx: usize) {
        let upper = token.upper();

        match token.kind {
            TokenKind::Keyword(SqlKeywordKind::Statement) => {
                self.process_statement_keyword(&upper);
            }
            TokenKind::Keyword(SqlKeywordKind::Clause) => {
                self.process_clause_keyword(&upper, tokens, idx);
            }
            TokenKind::Keyword(SqlKeywordKind::Join) => {
                self.process_join_keyword(&upper);
            }
            TokenKind::Identifier => {
                self.process_identifier(token, tokens, idx);
            }
            TokenKind::Star if self.clause == ClauseKind::SelectColumns => {
                // SELECT * — next must be FROM
                self.expecting = Expecting::FromKeyword;
            }
            _ => {}
        }
    }

    fn process_statement_keyword(&mut self, kw: &str) {
        match kw {
            "SELECT" => {
                if self.in_subquery {
                    // Don't change main statement for subqueries
                } else {
                    self.statement = Some(StatementKind::Select);
                }
                self.clause = ClauseKind::SelectColumns;
                self.expecting = Expecting::ColumnOrStar;
            }
            "INSERT" => {
                self.statement = Some(StatementKind::Insert);
                self.clause = ClauseKind::None;
                self.expecting = Expecting::IntoKeyword;
            }
            "UPDATE" => {
                self.statement = Some(StatementKind::Update);
                self.clause = ClauseKind::None;
                self.expecting = Expecting::TableName;
            }
            "DELETE" => {
                self.statement = Some(StatementKind::Delete);
                self.clause = ClauseKind::None;
                self.expecting = Expecting::FromKeyword;
            }
            "CREATE" | "DROP" | "ALTER" | "TRUNCATE" => {
                self.statement = Some(StatementKind::Ddl);
                self.clause = ClauseKind::None;
                self.expecting = Expecting::DdlObject;
            }
            _ => {}
        }
    }

    fn process_clause_keyword(&mut self, kw: &str, tokens: &[Token], idx: usize) {
        match kw {
            "FROM" => {
                self.clause = ClauseKind::From;
                self.expecting = Expecting::TableName;
            }
            "WHERE" => {
                self.clause = ClauseKind::Where;
                self.expecting = Expecting::ColumnOrExpression;
            }
            "INTO" if self.statement == Some(StatementKind::Insert) => {
                self.clause = ClauseKind::InsertInto;
                self.expecting = Expecting::TableName;
            }
            "SET" if self.statement == Some(StatementKind::Update) => {
                self.clause = ClauseKind::Set;
                self.expecting = Expecting::ColumnName;
            }
            "VALUES" => {
                self.clause = ClauseKind::Values;
                self.expecting = Expecting::ValueList;
            }
            "ORDER" if idx + 1 < tokens.len() && tokens[idx + 1].is_keyword("BY") => {
                self.clause = ClauseKind::OrderBy;
                self.expecting = Expecting::ColumnName;
            }
            "GROUP" if idx + 1 < tokens.len() && tokens[idx + 1].is_keyword("BY") => {
                self.clause = ClauseKind::GroupBy;
                self.expecting = Expecting::ColumnName;
            }
            "BY" => {
                // Already handled with ORDER/GROUP
            }
            "HAVING" => {
                self.clause = ClauseKind::Having;
                self.expecting = Expecting::AggregateOrColumn;
            }
            "ON" if self.clause == ClauseKind::Join => {
                self.clause = ClauseKind::JoinOn;
                self.expecting = Expecting::JoinCondition;
            }
            "AS" => {
                // Next token will be an alias
                self.expecting = Expecting::Alias;
            }
            "LIMIT" => {
                self.clause = ClauseKind::Limit;
                self.expecting = Expecting::Number;
            }
            "OFFSET" => {
                self.clause = ClauseKind::Offset;
                self.expecting = Expecting::Number;
            }
            _ => {}
        }
    }

    fn process_join_keyword(&mut self, kw: &str) {
        match kw {
            "JOIN" => {
                self.clause = ClauseKind::Join;
                self.expecting = Expecting::TableName;
            }
            "LEFT" | "RIGHT" | "INNER" | "OUTER" | "FULL" | "CROSS" | "NATURAL" => {
                // Join modifier - wait for JOIN keyword
                self.clause = ClauseKind::JoinPending;
            }
            _ => {}
        }
    }

    fn process_identifier(&mut self, token: &Token, tokens: &[Token], idx: usize) {
        let name = token.text.clone();

        // Check for table context based on statement type when clause is None
        let is_table_context = match self.clause {
            ClauseKind::From | ClauseKind::Join | ClauseKind::InsertInto => true,
            ClauseKind::None => {
                // UPDATE users -> users is a table
                // DELETE FROM users -> handled by From clause
                self.statement == Some(StatementKind::Update)
            }
            _ => false,
        };

        if is_table_context {
            // This is a table name
            self.tables.push(name.clone());

            // Check for alias (next token is identifier or AS + identifier)
            if idx + 1 < tokens.len() {
                let next = &tokens[idx + 1];
                if next.is_keyword("AS") && idx + 2 < tokens.len() {
                    let alias_token = &tokens[idx + 2];
                    if alias_token.is_identifier() {
                        self.aliases.insert(alias_token.text.clone(), name);
                    }
                } else if next.is_identifier() && !next.is_any_keyword() {
                    // Implicit alias: FROM users u
                    self.aliases.insert(next.text.clone(), name);
                }
            }

            // Update expectation based on context
            match self.clause {
                ClauseKind::From => {
                    self.expecting = Expecting::ClauseOrJoin;
                }
                ClauseKind::Join => {
                    self.expecting = Expecting::OnOrClause;
                }
                ClauseKind::InsertInto => {
                    self.expecting = Expecting::ValuesOrColumns;
                }
                ClauseKind::None if self.statement == Some(StatementKind::Update) => {
                    // After UPDATE table -> expect SET
                    self.clause = ClauseKind::None;
                    self.expecting = Expecting::SetKeyword;
                }
                _ => {}
            }
        } else {
            // Handle other identifier contexts
            match self.clause {
                ClauseKind::SelectColumns => {
                    // Column in SELECT
                    self.expecting = Expecting::CommaOrFrom;
                }
                ClauseKind::Where | ClauseKind::JoinOn => {
                    // Column in condition
                    self.expecting = Expecting::Operator;
                }
                ClauseKind::Set => {
                    // Column in SET
                    self.expecting = Expecting::Equals;
                }
                ClauseKind::OrderBy | ClauseKind::GroupBy => {
                    // Column in ORDER BY / GROUP BY
                    self.expecting = Expecting::OrderModifierOrComma;
                }
                _ => {}
            }
        }
    }

    fn determine_expectation(&mut self) {
        // Adjust expectation based on last token
        if let Some(ref token) = self.last_token {
            match token.kind {
                TokenKind::Comma => match self.clause {
                    ClauseKind::SelectColumns => self.expecting = Expecting::ColumnOrStar,
                    ClauseKind::From => self.expecting = Expecting::TableName,
                    ClauseKind::OrderBy | ClauseKind::GroupBy => {
                        self.expecting = Expecting::ColumnName
                    }
                    ClauseKind::Set => self.expecting = Expecting::ColumnName,
                    _ => {}
                },
                TokenKind::Operator if token.text == "=" => match self.clause {
                    ClauseKind::Where | ClauseKind::JoinOn => self.expecting = Expecting::Value,
                    ClauseKind::Set => self.expecting = Expecting::Value,
                    _ => {}
                },
                TokenKind::Keyword(SqlKeywordKind::Logical) if self.clause == ClauseKind::Where => {
                    self.expecting = Expecting::ColumnOrExpression;
                }
                TokenKind::Dot => {
                    // After a dot, we expect a column name (qualified name)
                    self.expecting = Expecting::QualifiedColumn;
                }
                _ => {}
            }
        }

        // If no statement yet, expect statement keyword
        if self.statement.is_none() && !self.in_subquery {
            self.expecting = Expecting::Statement;
        }
    }

    /// Get the table name for an alias
    pub fn resolve_alias(&self, alias: &str) -> Option<&String> {
        self.aliases.get(alias)
    }

    /// Check if a table is in scope
    pub fn has_table(&self, name: &str) -> bool {
        let lower = name.to_lowercase();
        self.tables.iter().any(|t| t.to_lowercase() == lower) || self.aliases.contains_key(name)
    }
}

/// Type of SQL statement
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Select,
    Insert,
    Update,
    Delete,
    Ddl,
}

/// Current clause within a statement
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseKind {
    None,
    SelectColumns,
    From,
    Where,
    Join,
    JoinPending, // After LEFT/RIGHT/etc, waiting for JOIN
    JoinOn,
    GroupBy,
    OrderBy,
    Having,
    Limit,
    Offset,
    InsertInto,
    InsertColumns,
    Values,
    Set,
}

/// What kind of token/completion we expect next
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expecting {
    /// Start of statement (SELECT, INSERT, etc.)
    Statement,
    /// Column name, *, or function
    ColumnOrStar,
    /// Just a column name
    ColumnName,
    /// Column or expression
    ColumnOrExpression,
    /// Column after dot (table.column)
    QualifiedColumn,
    /// FROM keyword (after SELECT *)
    FromKeyword,
    /// INTO keyword (after INSERT)
    IntoKeyword,
    /// Table name
    TableName,
    /// Comma or FROM
    CommaOrFrom,
    /// Clause keyword or JOIN
    ClauseOrJoin,
    /// ON keyword or other clause
    OnOrClause,
    /// Comparison operator (=, <, >, etc.)
    Operator,
    /// Value (string, number, etc.)
    Value,
    /// An alias name
    Alias,
    /// ASC/DESC or comma
    OrderModifierOrComma,
    /// Aggregate function or column (for HAVING)
    AggregateOrColumn,
    /// VALUES keyword or column list
    ValuesOrColumns,
    /// Opening paren for values
    ValueList,
    /// Equals sign (for SET)
    Equals,
    /// A number (for LIMIT/OFFSET)
    Number,
    /// DDL object type (TABLE, INDEX, etc.)
    DdlObject,
    /// Join condition columns
    JoinCondition,
    /// SET keyword (after UPDATE table)
    SetKeyword,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::sql_completion::tokenizer::SqlTokenizer;

    fn analyze(sql: &str) -> SqlContext {
        let tokens = SqlTokenizer::tokenize(sql);
        SqlContext::from_tokens(&tokens)
    }

    #[test]
    fn test_empty_input() {
        let ctx = analyze("");
        assert_eq!(ctx.statement, None);
        assert_eq!(ctx.expecting, Expecting::Statement);
    }

    #[test]
    fn test_select_start() {
        let ctx = analyze("SELECT");
        assert_eq!(ctx.statement, Some(StatementKind::Select));
        assert_eq!(ctx.expecting, Expecting::ColumnOrStar);
    }

    #[test]
    fn test_select_star() {
        let ctx = analyze("SELECT *");
        assert_eq!(ctx.statement, Some(StatementKind::Select));
        assert_eq!(ctx.expecting, Expecting::FromKeyword);
    }

    #[test]
    fn test_select_star_from() {
        let ctx = analyze("SELECT * FROM");
        assert_eq!(ctx.clause, ClauseKind::From);
        assert_eq!(ctx.expecting, Expecting::TableName);
    }

    #[test]
    fn test_select_from_table() {
        let ctx = analyze("SELECT * FROM users");
        assert!(ctx.tables.contains(&"users".to_string()));
        assert_eq!(ctx.expecting, Expecting::ClauseOrJoin);
    }

    #[test]
    fn test_select_from_table_where() {
        let ctx = analyze("SELECT * FROM users WHERE");
        assert_eq!(ctx.clause, ClauseKind::Where);
        assert_eq!(ctx.expecting, Expecting::ColumnOrExpression);
    }

    #[test]
    fn test_where_column() {
        let ctx = analyze("SELECT * FROM users WHERE id");
        assert_eq!(ctx.expecting, Expecting::Operator);
    }

    #[test]
    fn test_where_operator() {
        let ctx = analyze("SELECT * FROM users WHERE id =");
        assert_eq!(ctx.expecting, Expecting::Value);
    }

    #[test]
    fn test_table_alias() {
        let ctx = analyze("SELECT * FROM users u");
        assert!(ctx.aliases.contains_key("u"));
        assert_eq!(ctx.aliases.get("u"), Some(&"users".to_string()));
    }

    #[test]
    fn test_table_alias_with_as() {
        let ctx = analyze("SELECT * FROM users AS u");
        assert!(ctx.aliases.contains_key("u"));
        assert_eq!(ctx.aliases.get("u"), Some(&"users".to_string()));
    }

    #[test]
    fn test_insert() {
        let ctx = analyze("INSERT");
        assert_eq!(ctx.statement, Some(StatementKind::Insert));
        assert_eq!(ctx.expecting, Expecting::IntoKeyword);
    }

    #[test]
    fn test_insert_into() {
        let ctx = analyze("INSERT INTO");
        assert_eq!(ctx.clause, ClauseKind::InsertInto);
        assert_eq!(ctx.expecting, Expecting::TableName);
    }

    #[test]
    fn test_insert_into_table() {
        let ctx = analyze("INSERT INTO users");
        assert!(ctx.tables.contains(&"users".to_string()));
        assert_eq!(ctx.expecting, Expecting::ValuesOrColumns);
    }

    #[test]
    fn test_update() {
        let ctx = analyze("UPDATE");
        assert_eq!(ctx.statement, Some(StatementKind::Update));
        assert_eq!(ctx.expecting, Expecting::TableName);
    }

    #[test]
    fn test_update_table() {
        let ctx = analyze("UPDATE users");
        assert!(ctx.tables.contains(&"users".to_string()));
    }

    #[test]
    fn test_update_set() {
        let ctx = analyze("UPDATE users SET");
        assert_eq!(ctx.clause, ClauseKind::Set);
        assert_eq!(ctx.expecting, Expecting::ColumnName);
    }

    #[test]
    fn test_delete() {
        let ctx = analyze("DELETE");
        assert_eq!(ctx.statement, Some(StatementKind::Delete));
        assert_eq!(ctx.expecting, Expecting::FromKeyword);
    }

    #[test]
    fn test_delete_from() {
        let ctx = analyze("DELETE FROM");
        assert_eq!(ctx.clause, ClauseKind::From);
        assert_eq!(ctx.expecting, Expecting::TableName);
    }

    #[test]
    fn test_join() {
        let ctx = analyze("SELECT * FROM users JOIN");
        assert_eq!(ctx.clause, ClauseKind::Join);
        assert_eq!(ctx.expecting, Expecting::TableName);
    }

    #[test]
    fn test_join_table_on() {
        let ctx = analyze("SELECT * FROM users JOIN tickets ON");
        assert_eq!(ctx.clause, ClauseKind::JoinOn);
        assert_eq!(ctx.expecting, Expecting::JoinCondition);
    }

    #[test]
    fn test_order_by() {
        let ctx = analyze("SELECT * FROM users ORDER BY");
        assert_eq!(ctx.clause, ClauseKind::OrderBy);
        assert_eq!(ctx.expecting, Expecting::ColumnName);
    }

    #[test]
    fn test_semicolon_resets() {
        let ctx = analyze("SELECT * FROM users; SELECT");
        assert_eq!(ctx.statement, Some(StatementKind::Select));
        assert!(ctx.tables.is_empty()); // Tables from previous statement cleared
    }

    #[test]
    fn test_paren_depth() {
        let ctx = analyze("SELECT * FROM users WHERE id IN (");
        assert_eq!(ctx.paren_depth, 1);
    }

    #[test]
    fn test_resolve_alias() {
        let ctx = analyze("SELECT u.id FROM users u WHERE");
        assert_eq!(ctx.resolve_alias("u"), Some(&"users".to_string()));
    }

    #[test]
    fn test_select_column_comma() {
        let ctx = analyze("SELECT id,");
        assert_eq!(ctx.expecting, Expecting::ColumnOrStar);
    }

    #[test]
    fn test_qualified_name() {
        let ctx = analyze("SELECT u.");
        assert_eq!(ctx.expecting, Expecting::QualifiedColumn);
    }

    #[test]
    fn test_where_and() {
        let ctx = analyze("SELECT * FROM users WHERE id = 1 AND");
        assert_eq!(ctx.expecting, Expecting::ColumnOrExpression);
    }
}
