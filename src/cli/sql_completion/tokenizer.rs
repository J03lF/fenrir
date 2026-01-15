//! SQL Tokenizer for completion.
//!
//! This tokenizer is designed for SQL completion, not full SQL parsing.
//! It handles:
//! - Keywords and identifiers
//! - Strings (single and double quoted)
//! - Numbers (integers and decimals)
//! - Operators (single and multi-character)
//! - Punctuation (parentheses, commas, semicolons)
//! - Comments (-- and /* */)
//! - Partial tokens at end of input (for completion)

use super::keywords;

/// A SQL token with its position in the source
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub start: usize,
    pub end: usize,
}

impl Token {
    pub fn new(kind: TokenKind, text: impl Into<String>, start: usize, end: usize) -> Self {
        Self {
            kind,
            text: text.into(),
            start,
            end,
        }
    }
    
    /// Get the uppercase text (for keyword comparison)
    pub fn upper(&self) -> String {
        self.text.to_uppercase()
    }
    
    /// Check if this token is a specific keyword (case-insensitive)
    pub fn is_keyword(&self, kw: &str) -> bool {
        matches!(self.kind, TokenKind::Keyword(_)) && self.upper() == kw.to_uppercase()
    }
    
    /// Check if this is any keyword
    pub fn is_any_keyword(&self) -> bool {
        matches!(self.kind, TokenKind::Keyword(_))
    }
    
    /// Check if this is an identifier (table/column name)
    pub fn is_identifier(&self) -> bool {
        matches!(self.kind, TokenKind::Identifier)
    }
}

/// Token types recognized by the SQL tokenizer
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// SQL keyword (SELECT, FROM, WHERE, etc.)
    Keyword(SqlKeywordKind),
    /// Identifier (table name, column name, alias)
    Identifier,
    /// String literal ('value' or "value")
    String,
    /// Numeric literal (123, 45.67)
    Number,
    /// Operator (=, <>, >=, AND, OR, etc.)
    Operator,
    /// Star/asterisk (*)
    Star,
    /// Dot for qualified names (schema.table.column)
    Dot,
    /// Comma separator
    Comma,
    /// Semicolon statement separator
    Semicolon,
    /// Open parenthesis
    OpenParen,
    /// Close parenthesis
    CloseParen,
    /// Comment (-- or /* */)
    Comment,
    /// Whitespace (kept for position tracking)
    Whitespace,
    /// Partial/incomplete token at end of input
    Partial,
}

/// SQL keyword categories for context tracking
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SqlKeywordKind {
    /// SELECT, INSERT, UPDATE, DELETE, etc.
    Statement,
    /// FROM, WHERE, ORDER BY, etc.
    Clause,
    /// JOIN, LEFT, RIGHT, etc.
    Join,
    /// AND, OR, NOT
    Logical,
    /// ASC, DESC, NULLS, etc.
    Modifier,
    /// COUNT, SUM, AVG, etc.
    Function,
    /// CREATE, DROP, ALTER, etc.
    Ddl,
    /// INT, VARCHAR, etc.
    DataType,
    /// Other SQL keywords
    Other,
}

/// SQL Tokenizer that produces tokens from input
pub struct SqlTokenizer<'a> {
    #[allow(dead_code)]
    input: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    position: usize,
}

impl<'a> SqlTokenizer<'a> {
    /// Create a new tokenizer for the given input
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            chars: input.char_indices().peekable(),
            position: 0,
        }
    }
    
    /// Tokenize the entire input and return all tokens
    pub fn tokenize(input: &str) -> Vec<Token> {
        let mut tokenizer = SqlTokenizer::new(input);
        let mut tokens = Vec::new();
        
        while let Some(token) = tokenizer.next_token() {
            // Skip whitespace tokens for simpler processing
            if token.kind != TokenKind::Whitespace {
                tokens.push(token);
            }
        }
        
        tokens
    }
    
    /// Tokenize and return tokens with their positions
    /// Includes whitespace for accurate position tracking
    pub fn tokenize_with_whitespace(input: &str) -> Vec<Token> {
        let mut tokenizer = SqlTokenizer::new(input);
        let mut tokens = Vec::new();
        
        while let Some(token) = tokenizer.next_token() {
            tokens.push(token);
        }
        
        tokens
    }
    
    /// Get the next token from the input
    fn next_token(&mut self) -> Option<Token> {
        let (start, ch) = self.chars.next()?;
        self.position = start;
        
        match ch {
            // Whitespace
            ' ' | '\t' | '\n' | '\r' => self.read_whitespace(start, ch),
            
            // Single-character tokens
            '(' => Some(Token::new(TokenKind::OpenParen, "(", start, start + 1)),
            ')' => Some(Token::new(TokenKind::CloseParen, ")", start, start + 1)),
            ',' => Some(Token::new(TokenKind::Comma, ",", start, start + 1)),
            ';' => Some(Token::new(TokenKind::Semicolon, ";", start, start + 1)),
            '*' => Some(Token::new(TokenKind::Star, "*", start, start + 1)),
            '.' => Some(Token::new(TokenKind::Dot, ".", start, start + 1)),
            
            // Strings
            '\'' => self.read_string(start, '\''),
            '"' => self.read_quoted_identifier(start),
            
            // Comments or operators
            '-' => self.read_minus_or_comment(start),
            '/' => self.read_slash_or_comment(start),
            
            // Multi-char operators
            '<' => self.read_less_than(start),
            '>' => self.read_greater_than(start),
            '!' => self.read_bang(start),
            '=' => Some(Token::new(TokenKind::Operator, "=", start, start + 1)),
            '+' => Some(Token::new(TokenKind::Operator, "+", start, start + 1)),
            '%' => Some(Token::new(TokenKind::Operator, "%", start, start + 1)),
            '|' => self.read_pipe(start),
            ':' => self.read_colon(start),
            
            // Numbers
            '0'..='9' => self.read_number(start, ch),
            
            // Identifiers and keywords
            'a'..='z' | 'A'..='Z' | '_' => self.read_identifier(start, ch),
            
            // Anything else - treat as partial/unknown
            _ => Some(Token::new(TokenKind::Partial, ch.to_string(), start, start + 1)),
        }
    }
    
    fn read_whitespace(&mut self, start: usize, first: char) -> Option<Token> {
        let mut text = first.to_string();
        let mut end = start + first.len_utf8();
        
        while let Some(&(pos, ch)) = self.chars.peek() {
            if ch.is_whitespace() {
                text.push(ch);
                end = pos + ch.len_utf8();
                self.chars.next();
            } else {
                break;
            }
        }
        
        Some(Token::new(TokenKind::Whitespace, text, start, end))
    }
    
    fn read_string(&mut self, start: usize, quote: char) -> Option<Token> {
        let mut text = quote.to_string();
        let mut end = start + 1;
        let mut closed = false;
        
        while let Some((pos, ch)) = self.chars.next() {
            text.push(ch);
            end = pos + ch.len_utf8();
            
            if ch == quote {
                // Check for escaped quote ('')
                if let Some(&(_, next)) = self.chars.peek() {
                    if next == quote {
                        let (p, c) = self.chars.next().unwrap();
                        text.push(c);
                        end = p + c.len_utf8();
                        continue;
                    }
                }
                closed = true;
                break;
            }
        }
        
        // If string is not closed, it's partial (user still typing)
        let kind = if closed { TokenKind::String } else { TokenKind::Partial };
        Some(Token::new(kind, text, start, end))
    }
    
    fn read_quoted_identifier(&mut self, start: usize) -> Option<Token> {
        let mut text = "\"".to_string();
        let mut end = start + 1;
        let mut closed = false;
        
        while let Some((pos, ch)) = self.chars.next() {
            text.push(ch);
            end = pos + ch.len_utf8();
            
            if ch == '"' {
                // Check for escaped quote
                if let Some(&(_, next)) = self.chars.peek() {
                    if next == '"' {
                        let (p, c) = self.chars.next().unwrap();
                        text.push(c);
                        end = p + c.len_utf8();
                        continue;
                    }
                }
                closed = true;
                break;
            }
        }
        
        let kind = if closed { TokenKind::Identifier } else { TokenKind::Partial };
        Some(Token::new(kind, text, start, end))
    }
    
    fn read_minus_or_comment(&mut self, start: usize) -> Option<Token> {
        // Check for -- comment
        if let Some(&(_, '-')) = self.chars.peek() {
            self.chars.next();
            let mut text = "--".to_string();
            let mut end = start + 2;
            
            // Read until end of line
            for (pos, ch) in self.chars.by_ref() {
                end = pos + ch.len_utf8();
                if ch == '\n' {
                    break;
                }
                text.push(ch);
            }
            
            return Some(Token::new(TokenKind::Comment, text, start, end));
        }
        
        // Check for negative number
        if let Some(&(_, ch)) = self.chars.peek() {
            if ch.is_ascii_digit() {
                let (pos, digit) = self.chars.next().unwrap();
                return self.read_number_continuing(start, format!("-{}", digit), pos + 1);
            }
        }
        
        // Just a minus operator
        Some(Token::new(TokenKind::Operator, "-", start, start + 1))
    }
    
    fn read_slash_or_comment(&mut self, start: usize) -> Option<Token> {
        // Check for /* */ comment
        if let Some(&(_, '*')) = self.chars.peek() {
            self.chars.next();
            let mut text = "/*".to_string();
            let mut end = start + 2;
            let mut closed = false;
            
            while let Some((pos, ch)) = self.chars.next() {
                text.push(ch);
                end = pos + ch.len_utf8();
                
                if ch == '*' {
                    if let Some(&(_, '/')) = self.chars.peek() {
                        let (p, c) = self.chars.next().unwrap();
                        text.push(c);
                        end = p + c.len_utf8();
                        closed = true;
                        break;
                    }
                }
            }
            
            let kind = if closed { TokenKind::Comment } else { TokenKind::Partial };
            return Some(Token::new(kind, text, start, end));
        }
        
        // Just a division operator
        Some(Token::new(TokenKind::Operator, "/", start, start + 1))
    }
    
    fn read_less_than(&mut self, start: usize) -> Option<Token> {
        match self.chars.peek() {
            Some(&(_, '=')) => {
                self.chars.next();
                Some(Token::new(TokenKind::Operator, "<=", start, start + 2))
            }
            Some(&(_, '>')) => {
                self.chars.next();
                Some(Token::new(TokenKind::Operator, "<>", start, start + 2))
            }
            Some(&(_, '<')) => {
                self.chars.next();
                Some(Token::new(TokenKind::Operator, "<<", start, start + 2))
            }
            _ => Some(Token::new(TokenKind::Operator, "<", start, start + 1)),
        }
    }
    
    fn read_greater_than(&mut self, start: usize) -> Option<Token> {
        match self.chars.peek() {
            Some(&(_, '=')) => {
                self.chars.next();
                Some(Token::new(TokenKind::Operator, ">=", start, start + 2))
            }
            Some(&(_, '>')) => {
                self.chars.next();
                Some(Token::new(TokenKind::Operator, ">>", start, start + 2))
            }
            _ => Some(Token::new(TokenKind::Operator, ">", start, start + 1)),
        }
    }
    
    fn read_bang(&mut self, start: usize) -> Option<Token> {
        if let Some(&(_, '=')) = self.chars.peek() {
            self.chars.next();
            Some(Token::new(TokenKind::Operator, "!=", start, start + 2))
        } else {
            Some(Token::new(TokenKind::Operator, "!", start, start + 1))
        }
    }
    
    fn read_pipe(&mut self, start: usize) -> Option<Token> {
        if let Some(&(_, '|')) = self.chars.peek() {
            self.chars.next();
            Some(Token::new(TokenKind::Operator, "||", start, start + 2))
        } else {
            Some(Token::new(TokenKind::Operator, "|", start, start + 1))
        }
    }
    
    fn read_colon(&mut self, start: usize) -> Option<Token> {
        if let Some(&(_, ':')) = self.chars.peek() {
            self.chars.next();
            Some(Token::new(TokenKind::Operator, "::", start, start + 2))
        } else {
            // Could be a parameter placeholder :param
            Some(Token::new(TokenKind::Operator, ":", start, start + 1))
        }
    }
    
    fn read_number(&mut self, start: usize, first: char) -> Option<Token> {
        let text = first.to_string();
        self.read_number_continuing(start, text, start + 1)
    }
    
    fn read_number_continuing(&mut self, start: usize, mut text: String, mut end: usize) -> Option<Token> {
        let mut has_dot = false;
        let mut has_e = false;
        
        while let Some(&(pos, ch)) = self.chars.peek() {
            match ch {
                '0'..='9' => {
                    text.push(ch);
                    end = pos + 1;
                    self.chars.next();
                }
                '.' if !has_dot && !has_e => {
                    // Check for decimal number, not qualified name
                    has_dot = true;
                    text.push(ch);
                    end = pos + 1;
                    self.chars.next();
                }
                'e' | 'E' if !has_e => {
                    has_e = true;
                    text.push(ch);
                    end = pos + 1;
                    self.chars.next();
                    // Check for sign after E
                    if let Some(&(pos2, sign)) = self.chars.peek() {
                        if sign == '+' || sign == '-' {
                            text.push(sign);
                            end = pos2 + 1;
                            self.chars.next();
                        }
                    }
                }
                _ => break,
            }
        }
        
        Some(Token::new(TokenKind::Number, text, start, end))
    }
    
    fn read_identifier(&mut self, start: usize, first: char) -> Option<Token> {
        let mut text = first.to_string();
        let mut end = start + first.len_utf8();
        
        while let Some(&(pos, ch)) = self.chars.peek() {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
                text.push(ch);
                end = pos + ch.len_utf8();
                self.chars.next();
            } else {
                break;
            }
        }
        
        // Determine if this is a keyword or identifier
        let kind = self.classify_word(&text);
        Some(Token::new(kind, text, start, end))
    }
    
    /// Classify a word as keyword or identifier
    fn classify_word(&self, word: &str) -> TokenKind {
        let upper = word.to_uppercase();
        
        // Statement keywords
        if keywords::STATEMENT_KEYWORDS.contains(&upper.as_str()) {
            return TokenKind::Keyword(SqlKeywordKind::Statement);
        }
        
        // Join keywords
        if keywords::JOIN_TYPES.contains(&upper.as_str()) {
            return TokenKind::Keyword(SqlKeywordKind::Join);
        }
        
        // Clause keywords
        if matches!(upper.as_str(), 
            "FROM" | "WHERE" | "HAVING" | "ORDER" | "GROUP" | "BY" |
            "LIMIT" | "OFFSET" | "ON" | "USING" | "AS" | "INTO" | "SET" | "VALUES"
        ) {
            return TokenKind::Keyword(SqlKeywordKind::Clause);
        }
        
        // Logical operators
        if keywords::LOGICAL_OPERATORS.contains(&upper.as_str()) {
            return TokenKind::Keyword(SqlKeywordKind::Logical);
        }
        
        // Modifiers
        if keywords::SELECT_MODIFIERS.contains(&upper.as_str()) 
            || keywords::ORDER_MODIFIERS.contains(&upper.as_str()) {
            return TokenKind::Keyword(SqlKeywordKind::Modifier);
        }
        
        // Functions
        if keywords::AGGREGATE_FUNCTIONS.contains(&upper.as_str())
            || keywords::SCALAR_FUNCTIONS.contains(&upper.as_str()) {
            return TokenKind::Keyword(SqlKeywordKind::Function);
        }
        
        // DDL keywords
        if keywords::CREATE_OBJECTS.contains(&upper.as_str())
            || keywords::ALTER_ACTIONS.contains(&upper.as_str()) {
            return TokenKind::Keyword(SqlKeywordKind::Ddl);
        }
        
        // Data types
        if keywords::DATA_TYPES.contains(&upper.as_str()) {
            return TokenKind::Keyword(SqlKeywordKind::DataType);
        }
        
        // Other keywords
        if matches!(upper.as_str(),
            "NULL" | "TRUE" | "FALSE" | "IN" | "LIKE" | "ILIKE" | "BETWEEN" |
            "IS" | "EXISTS" | "ANY" | "ALL" | "UNION" | "INTERSECT" | "EXCEPT" |
            "CASE" | "WHEN" | "THEN" | "ELSE" | "END" | "IF" | "DEFAULT"
        ) {
            return TokenKind::Keyword(SqlKeywordKind::Other);
        }
        
        // Default: identifier
        TokenKind::Identifier
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_simple_select() {
        let tokens = SqlTokenizer::tokenize("SELECT * FROM users");
        assert_eq!(tokens.len(), 4);
        assert!(tokens[0].is_keyword("SELECT"));
        assert_eq!(tokens[1].kind, TokenKind::Star);
        assert!(tokens[2].is_keyword("FROM"));
        assert_eq!(tokens[3].text, "users");
    }
    
    #[test]
    fn test_select_with_columns() {
        let tokens = SqlTokenizer::tokenize("SELECT id, name FROM users");
        assert_eq!(tokens.len(), 6);
        assert!(tokens[0].is_keyword("SELECT"));
        assert_eq!(tokens[1].text, "id");
        assert_eq!(tokens[2].kind, TokenKind::Comma);
        assert_eq!(tokens[3].text, "name");
    }
    
    #[test]
    fn test_string_literal() {
        let tokens = SqlTokenizer::tokenize("SELECT * FROM users WHERE name = 'test'");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::String && t.text == "'test'"));
    }
    
    #[test]
    fn test_escaped_string() {
        let tokens = SqlTokenizer::tokenize("SELECT 'it''s working'");
        let string_token = tokens.iter().find(|t| t.kind == TokenKind::String).unwrap();
        assert_eq!(string_token.text, "'it''s working'");
    }
    
    #[test]
    fn test_operators() {
        let tokens = SqlTokenizer::tokenize("WHERE id >= 1 AND id <> 5");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Operator && t.text == ">="));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Operator && t.text == "<>"));
    }
    
    #[test]
    fn test_partial_string() {
        let tokens = SqlTokenizer::tokenize("SELECT * FROM users WHERE name = 'test");
        let last = tokens.last().unwrap();
        assert_eq!(last.kind, TokenKind::Partial);
    }
    
    #[test]
    fn test_comment() {
        let tokens = SqlTokenizer::tokenize("SELECT * -- comment\nFROM users");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Comment));
    }
    
    #[test]
    fn test_qualified_name() {
        let tokens = SqlTokenizer::tokenize("SELECT schema.table.column");
        assert_eq!(tokens.len(), 6); // SELECT, schema, ., table, ., column
        assert_eq!(tokens[2].kind, TokenKind::Dot);
        assert_eq!(tokens[4].kind, TokenKind::Dot);
    }
    
    #[test]
    fn test_numbers() {
        let tokens = SqlTokenizer::tokenize("SELECT 42, 3.14, -5, 1e10");
        let nums: Vec<_> = tokens.iter().filter(|t| t.kind == TokenKind::Number).collect();
        assert_eq!(nums.len(), 4);
        assert_eq!(nums[0].text, "42");
        assert_eq!(nums[1].text, "3.14");
        assert_eq!(nums[2].text, "-5");
        assert_eq!(nums[3].text, "1e10");
    }
    
    #[test]
    fn test_empty_input() {
        let tokens = SqlTokenizer::tokenize("");
        assert!(tokens.is_empty());
    }
    
    #[test]
    fn test_only_whitespace() {
        let tokens = SqlTokenizer::tokenize("   \t\n  ");
        assert!(tokens.is_empty());
    }
    
    #[test]
    fn test_parentheses() {
        let tokens = SqlTokenizer::tokenize("SELECT COUNT(*)");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::OpenParen));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::CloseParen));
    }
    
    #[test]
    fn test_case_insensitive_keywords() {
        let tokens1 = SqlTokenizer::tokenize("SELECT");
        let tokens2 = SqlTokenizer::tokenize("select");
        let tokens3 = SqlTokenizer::tokenize("SeLeCt");
        
        assert!(tokens1[0].is_any_keyword());
        assert!(tokens2[0].is_any_keyword());
        assert!(tokens3[0].is_any_keyword());
    }
    
    #[test]
    fn test_insert_statement() {
        let tokens = SqlTokenizer::tokenize("INSERT INTO users (id, name) VALUES (1, 'test')");
        assert!(tokens[0].is_keyword("INSERT"));
        assert!(tokens[1].is_keyword("INTO"));
    }
    
    #[test]
    fn test_update_statement() {
        let tokens = SqlTokenizer::tokenize("UPDATE users SET name = 'new' WHERE id = 1");
        assert!(tokens[0].is_keyword("UPDATE"));
        assert!(tokens.iter().any(|t| t.is_keyword("SET")));
        assert!(tokens.iter().any(|t| t.is_keyword("WHERE")));
    }
    
    #[test]
    fn test_delete_statement() {
        let tokens = SqlTokenizer::tokenize("DELETE FROM users WHERE id = 1");
        assert!(tokens[0].is_keyword("DELETE"));
        assert!(tokens[1].is_keyword("FROM"));
    }
    
    #[test]
    fn test_join_keywords() {
        let tokens = SqlTokenizer::tokenize("SELECT * FROM a LEFT JOIN b ON a.id = b.a_id");
        assert!(tokens.iter().any(|t| t.is_keyword("LEFT")));
        assert!(tokens.iter().any(|t| t.is_keyword("JOIN")));
        assert!(tokens.iter().any(|t| t.is_keyword("ON")));
    }
    
    #[test]
    fn test_block_comment() {
        let tokens = SqlTokenizer::tokenize("SELECT /* comment */ * FROM users");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Comment));
        // Should still have SELECT, *, FROM, users
        assert!(tokens.iter().any(|t| t.is_keyword("SELECT")));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Star));
    }
    
    #[test]
    fn test_token_positions() {
        let input = "SELECT id";
        let tokens = SqlTokenizer::tokenize(input);
        assert_eq!(tokens[0].start, 0);
        assert_eq!(tokens[0].end, 6);
        assert_eq!(tokens[1].start, 7);
        assert_eq!(tokens[1].end, 9);
    }
}

