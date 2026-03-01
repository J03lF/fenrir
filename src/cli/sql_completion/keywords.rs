//! SQL Keywords organized by category and context.
//!
//! This module provides categorized SQL keywords for intelligent completion.
//! Keywords are organized by:
//! - Statement type (SELECT, INSERT, UPDATE, DELETE, DDL)
//! - Clause position (FROM, WHERE, ORDER BY, etc.)
//! - Operator type (comparison, logical, etc.)

use std::collections::HashSet;

/// All SQL keywords that can start a statement
pub const STATEMENT_KEYWORDS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "TRUNCATE", "BEGIN",
    "COMMIT", "ROLLBACK", "EXPLAIN", "ANALYZE", "GRANT", "REVOKE", "VACUUM", "REINDEX",
    "WITH", // CTE
];

/// Keywords valid after SELECT (before column list)
pub const SELECT_MODIFIERS: &[&str] = &["DISTINCT", "ALL"];

/// Aggregate functions
pub const AGGREGATE_FUNCTIONS: &[&str] = &[
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "ARRAY_AGG",
    "STRING_AGG",
    "JSON_AGG",
    "BOOL_AND",
    "BOOL_OR",
    "EVERY",
    "STDDEV",
    "VARIANCE",
];

/// Scalar functions commonly used in SELECT
pub const SCALAR_FUNCTIONS: &[&str] = &[
    "COALESCE",
    "NULLIF",
    "CAST",
    "CONVERT",
    "UPPER",
    "LOWER",
    "TRIM",
    "SUBSTRING",
    "LENGTH",
    "CONCAT",
    "NOW",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "DATE",
    "TIME",
    "EXTRACT",
    "DATE_TRUNC",
    "ABS",
    "ROUND",
    "CEIL",
    "FLOOR",
    "MOD",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
];

/// Keywords that follow a column in SELECT (before FROM)
pub const AFTER_SELECT_COLUMN: &[&str] = &["AS", "FROM"];

/// Keywords valid after FROM clause table
pub const AFTER_FROM_TABLE: &[&str] = &[
    "WHERE",
    "JOIN",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "CROSS",
    "NATURAL",
    "ON",
    "USING",
    "GROUP",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "AS", // Table alias
];

/// JOIN type keywords
pub const JOIN_TYPES: &[&str] = &[
    "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "NATURAL", "OUTER",
];

/// Comparison operators
pub const COMPARISON_OPERATORS: &[&str] = &[
    "=",
    "!=",
    "<>",
    "<",
    ">",
    "<=",
    ">=",
    "IN",
    "NOT IN",
    "LIKE",
    "ILIKE",
    "NOT LIKE",
    "BETWEEN",
    "IS",
    "IS NOT",
    "EXISTS",
    "NOT EXISTS",
    "ANY",
    "ALL",
    "SOME",
];

/// Logical operators for WHERE conditions
pub const LOGICAL_OPERATORS: &[&str] = &["AND", "OR", "NOT"];

/// NULL-related keywords
pub const NULL_KEYWORDS: &[&str] = &["NULL", "NOT NULL", "IS NULL", "IS NOT NULL"];

/// ORDER BY modifiers
pub const ORDER_MODIFIERS: &[&str] = &["ASC", "DESC", "NULLS", "FIRST", "LAST"];

/// Keywords for after UPDATE table
pub const AFTER_UPDATE_TABLE: &[&str] = &["SET"];

/// Keywords for after DELETE
pub const AFTER_DELETE: &[&str] = &["FROM"];

/// Keywords for after INSERT
pub const AFTER_INSERT: &[&str] = &["INTO"];

/// Keywords for after INSERT INTO table
pub const AFTER_INSERT_TABLE: &[&str] = &["VALUES", "SELECT", "DEFAULT"];

/// Keywords for after SET column = value
pub const AFTER_SET_VALUE: &[&str] = &["WHERE"];

/// DDL: CREATE object types
pub const CREATE_OBJECTS: &[&str] = &[
    "TABLE",
    "INDEX",
    "UNIQUE",
    "VIEW",
    "MATERIALIZED",
    "SCHEMA",
    "DATABASE",
    "SEQUENCE",
    "FUNCTION",
    "PROCEDURE",
    "TRIGGER",
    "TYPE",
    "EXTENSION",
    "ROLE",
    "USER",
];

/// DDL: DROP object types
pub const DROP_OBJECTS: &[&str] = &[
    "TABLE",
    "INDEX",
    "VIEW",
    "MATERIALIZED",
    "SCHEMA",
    "DATABASE",
    "SEQUENCE",
    "FUNCTION",
    "PROCEDURE",
    "TRIGGER",
    "TYPE",
    "EXTENSION",
    "ROLE",
    "USER",
    "IF", // IF EXISTS
];

/// DDL: ALTER actions
pub const ALTER_ACTIONS: &[&str] = &[
    "ADD",
    "DROP",
    "ALTER",
    "RENAME",
    "SET",
    "RESET",
    "COLUMN",
    "CONSTRAINT",
    "INDEX",
    "OWNER",
    "SCHEMA",
];

/// Data types for CREATE TABLE / CAST
pub const DATA_TYPES: &[&str] = &[
    "INT",
    "INTEGER",
    "BIGINT",
    "SMALLINT",
    "DECIMAL",
    "NUMERIC",
    "REAL",
    "DOUBLE",
    "FLOAT",
    "CHAR",
    "VARCHAR",
    "TEXT",
    "BYTEA",
    "BOOLEAN",
    "BOOL",
    "DATE",
    "TIME",
    "TIMESTAMP",
    "TIMESTAMPTZ",
    "INTERVAL",
    "UUID",
    "JSON",
    "JSONB",
    "XML",
    "ARRAY",
    "SERIAL",
    "BIGSERIAL",
];

/// Clause keywords (FROM, WHERE, etc.)
pub const CLAUSE_KEYWORDS: &[&str] = &[
    "FROM", "WHERE", "HAVING", "ORDER", "GROUP", "BY", "LIMIT", "OFFSET", "ON", "USING", "AS",
    "INTO", "SET", "VALUES",
];

/// Check if a string is a SQL keyword (case-insensitive)
pub fn is_keyword(word: &str) -> bool {
    let upper = word.to_uppercase();
    STATEMENT_KEYWORDS.contains(&upper.as_str())
        || SELECT_MODIFIERS.contains(&upper.as_str())
        || AGGREGATE_FUNCTIONS.contains(&upper.as_str())
        || SCALAR_FUNCTIONS.contains(&upper.as_str())
        || AFTER_FROM_TABLE.contains(&upper.as_str())
        || JOIN_TYPES.contains(&upper.as_str())
        || LOGICAL_OPERATORS.contains(&upper.as_str())
        || ORDER_MODIFIERS.contains(&upper.as_str())
        || CREATE_OBJECTS.contains(&upper.as_str())
        || DATA_TYPES.contains(&upper.as_str())
        || CLAUSE_KEYWORDS.contains(&upper.as_str())
}

/// Get all keywords as a HashSet for fast lookup
pub fn all_keywords() -> HashSet<String> {
    let mut set = HashSet::new();

    for &kw in STATEMENT_KEYWORDS {
        set.insert(kw.to_string());
    }
    for &kw in SELECT_MODIFIERS {
        set.insert(kw.to_string());
    }
    for &kw in AGGREGATE_FUNCTIONS {
        set.insert(kw.to_string());
    }
    for &kw in SCALAR_FUNCTIONS {
        set.insert(kw.to_string());
    }
    for &kw in AFTER_SELECT_COLUMN {
        set.insert(kw.to_string());
    }
    for &kw in AFTER_FROM_TABLE {
        set.insert(kw.to_string());
    }
    for &kw in JOIN_TYPES {
        set.insert(kw.to_string());
    }
    for &kw in COMPARISON_OPERATORS {
        set.insert(kw.to_string());
    }
    for &kw in LOGICAL_OPERATORS {
        set.insert(kw.to_string());
    }
    for &kw in NULL_KEYWORDS {
        set.insert(kw.to_string());
    }
    for &kw in ORDER_MODIFIERS {
        set.insert(kw.to_string());
    }
    for &kw in CREATE_OBJECTS {
        set.insert(kw.to_string());
    }
    for &kw in DROP_OBJECTS {
        set.insert(kw.to_string());
    }
    for &kw in ALTER_ACTIONS {
        set.insert(kw.to_string());
    }
    for &kw in DATA_TYPES {
        set.insert(kw.to_string());
    }

    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_keyword() {
        assert!(is_keyword("SELECT"));
        assert!(is_keyword("select"));
        assert!(is_keyword("Select"));
        assert!(is_keyword("FROM"));
        assert!(is_keyword("COUNT"));
        assert!(!is_keyword("users"));
        assert!(!is_keyword("my_table"));
    }

    #[test]
    fn test_all_keywords_not_empty() {
        let kws = all_keywords();
        assert!(kws.len() > 50, "Should have many keywords");
        assert!(kws.contains("SELECT"));
        assert!(kws.contains("FROM"));
        assert!(kws.contains("WHERE"));
    }
}
