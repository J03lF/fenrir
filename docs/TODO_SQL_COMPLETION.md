# TODO: Professionelle SQL Completion (Tier 1)

## Ziel
Intelligente, AST-aware SQL Completion wie in DataGrip/DBeaver.

## Architektur

```
┌─────────────────────────────────────────────────────────────────────┐
│                        SQL Completion Engine                        │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────────────┐  │
│  │   Tokenizer  │───▶│Context Stack │───▶│  Suggestion Engine   │  │
│  │  (Robust)    │    │  (SQL-aware) │    │  (Schema + Keywords) │  │
│  └──────────────┘    └──────────────┘    └──────────────────────┘  │
│         │                   │                      │               │
│         ▼                   ▼                      ▼               │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────────────┐  │
│  │  Handles:    │    │  Tracks:     │    │  Returns:            │  │
│  │  - Strings   │    │  - SELECT/   │    │  - Keywords          │  │
│  │  - Comments  │    │    FROM/etc  │    │  - Tables            │  │
│  │  - Parens    │    │  - Subquery  │    │  - Columns           │  │
│  │  - Operators │    │    depth     │    │  - Aliases           │  │
│  │  - Partial   │    │  - JOIN ctx  │    │  - Functions         │  │
│  │    tokens    │    │  - Aliases   │    │  - Operators         │  │
│  └──────────────┘    └──────────────┘    └──────────────────────┘  │
│                                                                     │
│  ┌──────────────────────────────────────────────────────────────┐  │
│  │                     Schema Cache                              │  │
│  │  - Tables: users, tickets, comments, ...                     │  │
│  │  - Columns: users.id, users.name, users.email, ...           │  │
│  │  - Aliases: u → users, t → tickets (per query)               │  │
│  └──────────────────────────────────────────────────────────────┘  │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Phase 1: Foundation (MUSS)

### 1.1 Robuster SQL Tokenizer
- [ ] Neuen `SqlTokenizer` struct erstellen
- [ ] Token-Typen definieren:
  ```rust
  enum SqlToken {
      Keyword(SqlKeyword),    // SELECT, FROM, WHERE, ...
      Identifier(String),     // table_name, column_name
      String(String),         // 'value', "value"
      Number(String),         // 123, 45.67
      Operator(String),       // =, <>, >=, AND, OR
      Punctuation(char),      // (, ), ,, ;
      Star,                   // *
      Dot,                    // .
      Whitespace,             // (ignored but tracked for position)
      Comment(String),        // -- comment, /* comment */
      Partial(String),        // Unvollständiges Token am Ende
  }
  ```
- [ ] Positionstracking (für Cursor-Position)
- [ ] Edge Cases:
  - Strings mit Escapes: `'it''s'`
  - Qualified names: `schema.table.column`
  - Negative numbers: `-42`
  - Multi-char operators: `<>`, `>=`, `!=`

### 1.2 Context Stack
- [ ] `SqlContext` struct:
  ```rust
  struct SqlContext {
      stack: Vec<ContextFrame>,
      aliases: HashMap<String, String>,  // alias → table
      current_tables: Vec<String>,        // Tables in current FROM
  }
  
  enum ContextFrame {
      Statement(StatementKind),  // SELECT, INSERT, UPDATE, DELETE
      Clause(ClauseKind),        // FROM, WHERE, ORDER BY, ...
      Subquery(Box<SqlContext>), // Nested context
      Parentheses,               // Generic ( ... )
      Function(String),          // COUNT( ... )
  }
  ```
- [ ] Push/Pop Logik für Kontext-Tracking
- [ ] Alias-Erkennung: `FROM users u` → u = users

### 1.3 Schema Cache
- [ ] `SchemaCache` struct:
  ```rust
  struct SchemaCache {
      tables: HashSet<String>,
      columns: HashMap<String, Vec<String>>,  // table → columns
      functions: Vec<String>,                  // Built-in functions
      last_refresh: Instant,
  }
  ```
- [ ] Spalten pro Tabelle aus DB laden
- [ ] Lazy Loading (bei Bedarf nachladen)
- [ ] Cache-Invalidierung nach DDL

---

## Phase 2: Suggestion Engine (MUSS)

### 2.1 Kontext-basierte Vorschläge
- [ ] Regeln für jeden Kontext:
  ```
  Statement::Select + Clause::None     → *, DISTINCT, columns, functions
  Statement::Select + Clause::Columns  → FROM, AS, ","
  Statement::Select + "*"              → FROM (NUR!)
  Statement::Select + Clause::From     → tables
  Statement::Select + after_table      → WHERE, JOIN, ORDER BY, ...
  Statement::Insert                    → INTO (NUR!)
  Statement::Insert + Clause::Into     → tables
  Statement::Update + after_table      → SET (NUR!)
  Statement::Delete                    → FROM (NUR!)
  ```

### 2.2 Spalten-Completion
- [ ] Nach FROM: Spalten der FROM-Tabellen anbieten
- [ ] Qualified: `users.` → columns of users
- [ ] Alias-aware: `u.` → columns of users (wenn u = users)
- [ ] JOIN-aware: Spalten aller gejointen Tabellen

### 2.3 Intelligente Filterung
- [ ] Prefix-Matching (case-insensitive)
- [ ] Fuzzy-Matching (optional): `usrs` → `users`
- [ ] Bereits verwendete Items dimmen/verstecken

---

## Phase 3: Advanced Features (NICE TO HAVE)

### 3.1 Subquery Support
- [ ] Kontext-Stack für verschachtelte Queries
- [ ] Korrelierte Subqueries (Zugriff auf äußere Tabellen)

### 3.2 CTE Support (WITH)
- [ ] CTE-Namen als "virtuelle Tabellen" tracken
- [ ] CTE-Spalten für Completion verfügbar machen

### 3.3 Window Functions
- [ ] `OVER (` Kontext erkennen
- [ ] `PARTITION BY`, `ORDER BY` vorschlagen

### 3.4 Type-Aware Suggestions
- [ ] Bei `WHERE status =` nur valide Werte vorschlagen
- [ ] Bei `ORDER BY` numerische Spalten bevorzugen

---

## Phase 4: Integration

### 4.1 Cycling/UI
- [ ] Grid-Anzeige beibehalten
- [ ] Kategorisierte Anzeige:
  ```
  ┌─ Tables ──────────────────┐
  │ users  tickets  comments  │
  ├─ Columns ─────────────────┤
  │ id  name  email  status   │
  ├─ Keywords ────────────────┤
  │ WHERE  JOIN  ORDER BY     │
  └───────────────────────────┘
  ```
- [ ] Robustes Cycling ohne Bugs

### 4.2 Performance
- [ ] Debounce bei schnellem Tippen
- [ ] Async Schema-Loading
- [ ] Caching von Completion-Ergebnissen

### 4.3 SSH Integration
- [ ] Dieselbe Engine für SSH-Shell
- [ ] Schema-Cache über SSH-Session halten

---

## Dateien & Module

```
src/cli/completion/
├── mod.rs              # Public API
├── tokenizer.rs        # SqlTokenizer
├── context.rs          # SqlContext, ContextFrame
├── schema.rs           # SchemaCache
├── engine.rs           # Suggestion Engine (Hauptlogik)
├── suggestions.rs      # Kontext → Vorschläge Mapping
├── keywords.rs         # SQL Keywords nach Kategorie
└── tests.rs            # Unit Tests
```

---

## Implementierungsreihenfolge

1. **Tokenizer** - Fundament für alles
2. **Context Stack** - Versteht SQL-Struktur
3. **Schema Cache** - Tabellen/Spalten laden
4. **Suggestion Engine** - Kontext → Vorschläge
5. **Integration** - In bestehende Completion einbauen
6. **Polish** - UI, Performance, Edge Cases

---

## Zeitschätzung

| Phase | Aufwand | Priorität |
|-------|---------|-----------|
| Phase 1: Foundation | ~4-6h | MUSS |
| Phase 2: Suggestions | ~3-4h | MUSS |
| Phase 3: Advanced | ~4-6h | NICE TO HAVE |
| Phase 4: Integration | ~2-3h | MUSS |
| **Total** | **~13-19h** | |

---

## Referenzen

- [sqlparser-rs](https://github.com/sqlparser-rs/sqlparser-rs) - Für Schema-Parsing (DDL)
- [DataGrip Completion](https://www.jetbrains.com/help/datagrip/auto-completing-code.html)
- [Monaco SQL Languages](https://github.com/nicknisi/monaco-sql-languages) - Web-basierte SQL Completion

---

---

## Phase 5: Testing & Validation (KRITISCH)

### 5.1 Test Infrastructure

```rust
// tests/unit/cli/sql_completion_tests.rs

/// Test helper für Completion-Tests
struct CompletionTestCase {
    input: &'static str,           // SQL bis zum Cursor
    cursor_pos: usize,             // Position des Cursors (default: Ende)
    tables: Vec<&'static str>,     // Verfügbare Tabellen
    columns: HashMap<&'static str, Vec<&'static str>>,  // Tabelle → Spalten
    expected: Vec<&'static str>,   // Erwartete Vorschläge
    not_expected: Vec<&'static str>, // Diese DÜRFEN NICHT erscheinen
    description: &'static str,     // Was wird getestet
}

impl CompletionTestCase {
    fn run(&self) -> TestResult {
        let engine = SqlCompletionEngine::new_test(
            self.tables.clone(),
            self.columns.clone()
        );
        let suggestions = engine.complete(self.input, self.cursor_pos);
        
        // Prüfe erwartete Vorschläge
        for exp in &self.expected {
            assert!(suggestions.contains(exp), 
                "Missing expected: {} in {:?}", exp, suggestions);
        }
        
        // Prüfe verbotene Vorschläge
        for not_exp in &self.not_expected {
            assert!(!suggestions.contains(not_exp),
                "Unexpected: {} in {:?}", not_exp, suggestions);
        }
    }
}
```

### 5.2 Comprehensive Test Cases

```rust
const TEST_CASES: &[CompletionTestCase] = &[
    // ═══════════════════════════════════════════════════════════════
    // EMPTY INPUT
    // ═══════════════════════════════════════════════════════════════
    CompletionTestCase {
        input: "",
        expected: &["SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP"],
        not_expected: &["FROM", "WHERE", "SET", "*"],
        description: "Empty input → statement keywords only",
    },
    CompletionTestCase {
        input: "S",
        expected: &["SELECT"],
        not_expected: &["INSERT", "UPDATE", "FROM"],
        description: "Partial 'S' → SELECT",
    },
    
    // ═══════════════════════════════════════════════════════════════
    // SELECT STATEMENT
    // ═══════════════════════════════════════════════════════════════
    CompletionTestCase {
        input: "SELECT ",
        expected: &["*", "DISTINCT"],
        not_expected: &["FROM", "WHERE", "SELECT"],
        description: "After SELECT → columns, *, DISTINCT (NOT FROM)",
    },
    CompletionTestCase {
        input: "SELECT *",
        expected: &["FROM"],
        not_expected: &["*", "DISTINCT", "WHERE", "SELECT", ","],
        description: "After SELECT * → ONLY FROM!",
    },
    CompletionTestCase {
        input: "SELECT * ",
        expected: &["FROM"],
        not_expected: &["*", "WHERE", "AND"],
        description: "After SELECT * (space) → ONLY FROM!",
    },
    CompletionTestCase {
        input: "SELECT * F",
        expected: &["FROM"],
        not_expected: &["FALSE", "FULL"],
        description: "After SELECT * F → FROM (not FALSE!)",
    },
    CompletionTestCase {
        input: "SELECT id, name ",
        expected: &["FROM", ","],
        not_expected: &["SELECT", "*", "WHERE"],
        description: "After columns → FROM or more columns",
    },
    CompletionTestCase {
        input: "SELECT id, ",
        expected: &["*"],  // More columns
        not_expected: &["FROM", "WHERE"],
        description: "After comma → more columns",
    },
    CompletionTestCase {
        input: "SELECT * FROM ",
        tables: &["users", "tickets", "comments"],
        expected: &["users", "tickets", "comments"],
        not_expected: &["SELECT", "FROM", "WHERE", "*"],
        description: "After FROM → tables only",
    },
    CompletionTestCase {
        input: "SELECT * FROM users ",
        expected: &["WHERE", "JOIN", "LEFT", "ORDER", "GROUP", "LIMIT"],
        not_expected: &["FROM", "SELECT", "*"],
        description: "After FROM table → clauses",
    },
    CompletionTestCase {
        input: "SELECT * FROM users WHERE ",
        columns: &[("users", &["id", "name", "email", "status"])],
        expected: &["id", "name", "email", "status"],
        not_expected: &["FROM", "SELECT", "WHERE"],
        description: "After WHERE → columns",
    },
    CompletionTestCase {
        input: "SELECT * FROM users WHERE id ",
        expected: &["=", "!=", "<>", "IN", "LIKE", "IS", "BETWEEN"],
        not_expected: &["AND", "OR", "FROM"],
        description: "After WHERE column → operators",
    },
    CompletionTestCase {
        input: "SELECT * FROM users WHERE id = 1 ",
        expected: &["AND", "OR", "ORDER", "LIMIT"],
        not_expected: &["=", "FROM", "WHERE"],
        description: "After WHERE condition → AND/OR/clauses",
    },
    
    // ═══════════════════════════════════════════════════════════════
    // INSERT STATEMENT
    // ═══════════════════════════════════════════════════════════════
    CompletionTestCase {
        input: "INSERT ",
        expected: &["INTO"],
        not_expected: &["SELECT", "VALUES", "FROM", "SET"],
        description: "After INSERT → ONLY INTO!",
    },
    CompletionTestCase {
        input: "INSERT INTO ",
        tables: &["users", "tickets"],
        expected: &["users", "tickets"],
        not_expected: &["INTO", "VALUES", "SELECT"],
        description: "After INSERT INTO → tables",
    },
    CompletionTestCase {
        input: "INSERT INTO users ",
        expected: &["VALUES", "("],
        not_expected: &["INTO", "SELECT", "SET"],
        description: "After INSERT INTO table → VALUES or (",
    },
    
    // ═══════════════════════════════════════════════════════════════
    // UPDATE STATEMENT
    // ═══════════════════════════════════════════════════════════════
    CompletionTestCase {
        input: "UPDATE ",
        tables: &["users", "tickets"],
        expected: &["users", "tickets"],
        not_expected: &["SET", "WHERE", "SELECT"],
        description: "After UPDATE → tables",
    },
    CompletionTestCase {
        input: "UPDATE users ",
        expected: &["SET"],
        not_expected: &["WHERE", "UPDATE", "SELECT", "FROM"],
        description: "After UPDATE table → ONLY SET!",
    },
    CompletionTestCase {
        input: "UPDATE users SET ",
        columns: &[("users", &["id", "name", "email"])],
        expected: &["id", "name", "email"],
        not_expected: &["SET", "WHERE", "UPDATE"],
        description: "After SET → columns",
    },
    CompletionTestCase {
        input: "UPDATE users SET name = 'test' ",
        expected: &[",", "WHERE"],
        not_expected: &["SET", "UPDATE", "="],
        description: "After SET value → comma or WHERE",
    },
    
    // ═══════════════════════════════════════════════════════════════
    // DELETE STATEMENT
    // ═══════════════════════════════════════════════════════════════
    CompletionTestCase {
        input: "DELETE ",
        expected: &["FROM"],
        not_expected: &["SELECT", "WHERE", "SET", "*"],
        description: "After DELETE → ONLY FROM!",
    },
    CompletionTestCase {
        input: "DELETE FROM ",
        tables: &["users", "tickets"],
        expected: &["users", "tickets"],
        not_expected: &["FROM", "DELETE", "WHERE"],
        description: "After DELETE FROM → tables",
    },
    CompletionTestCase {
        input: "DELETE FROM users ",
        expected: &["WHERE"],
        not_expected: &["FROM", "DELETE", "SET"],
        description: "After DELETE FROM table → WHERE",
    },
    
    // ═══════════════════════════════════════════════════════════════
    // JOIN
    // ═══════════════════════════════════════════════════════════════
    CompletionTestCase {
        input: "SELECT * FROM users JOIN ",
        tables: &["users", "tickets", "comments"],
        expected: &["tickets", "comments"],  // NOT users (already used)
        not_expected: &["JOIN", "ON", "WHERE"],
        description: "After JOIN → tables (excluding already used)",
    },
    CompletionTestCase {
        input: "SELECT * FROM users JOIN tickets ",
        expected: &["ON"],
        not_expected: &["JOIN", "WHERE", "FROM"],
        description: "After JOIN table → ON",
    },
    CompletionTestCase {
        input: "SELECT * FROM users u JOIN tickets t ON ",
        columns: &[("users", &["id", "name"]), ("tickets", &["id", "user_id"])],
        expected: &["u.id", "u.name", "t.id", "t.user_id"],
        not_expected: &["ON", "JOIN"],
        description: "After ON → qualified columns from both tables",
    },
    
    // ═══════════════════════════════════════════════════════════════
    // ALIASES
    // ═══════════════════════════════════════════════════════════════
    CompletionTestCase {
        input: "SELECT u.",
        tables: &["users"],
        columns: &[("users", &["id", "name", "email"])],
        aliases: &[("u", "users")],
        expected: &["id", "name", "email"],
        not_expected: &["u", "users", "SELECT"],
        description: "After alias. → columns of aliased table",
    },
    CompletionTestCase {
        input: "SELECT * FROM users u WHERE u.",
        columns: &[("users", &["id", "name", "email"])],
        expected: &["id", "name", "email"],
        description: "Alias in WHERE → columns",
    },
    
    // ═══════════════════════════════════════════════════════════════
    // ORDER BY / GROUP BY
    // ═══════════════════════════════════════════════════════════════
    CompletionTestCase {
        input: "SELECT * FROM users ORDER BY ",
        columns: &[("users", &["id", "name", "created_at"])],
        expected: &["id", "name", "created_at"],
        not_expected: &["ORDER", "BY", "FROM"],
        description: "After ORDER BY → columns",
    },
    CompletionTestCase {
        input: "SELECT * FROM users ORDER BY name ",
        expected: &["ASC", "DESC", ",", "LIMIT"],
        not_expected: &["ORDER", "BY", "FROM"],
        description: "After ORDER BY column → ASC/DESC/more",
    },
    CompletionTestCase {
        input: "SELECT * FROM users GROUP BY ",
        columns: &[("users", &["id", "status", "type"])],
        expected: &["id", "status", "type"],
        description: "After GROUP BY → columns",
    },
    CompletionTestCase {
        input: "SELECT * FROM users GROUP BY status ",
        expected: &["HAVING", ",", "ORDER"],
        not_expected: &["GROUP", "BY", "WHERE"],
        description: "After GROUP BY column → HAVING/ORDER",
    },
    
    // ═══════════════════════════════════════════════════════════════
    // EDGE CASES
    // ═══════════════════════════════════════════════════════════════
    CompletionTestCase {
        input: "SELECT * FROM users WHERE id IN (",
        expected: &["SELECT"],  // Subquery possible
        description: "After IN ( → subquery or values",
    },
    CompletionTestCase {
        input: "SELECT * FROM users WHERE name LIKE ",
        expected: &[],  // User types string
        not_expected: &["LIKE", "AND", "OR"],
        description: "After LIKE → no suggestions (user types pattern)",
    },
    CompletionTestCase {
        input: "SELECT * FROM users LIMIT ",
        expected: &[],  // User types number
        description: "After LIMIT → no suggestions (user types number)",
    },
    CompletionTestCase {
        input: "  SELECT  *  FROM  ",
        tables: &["users"],
        expected: &["users"],
        description: "Multiple spaces → still works",
    },
    CompletionTestCase {
        input: "select * from ",
        tables: &["users"],
        expected: &["users"],
        description: "Lowercase keywords → still works",
    },
    CompletionTestCase {
        input: "SELECT * FROM users; SELECT ",
        expected: &["*", "DISTINCT"],
        description: "After semicolon → new statement context",
    },
];
```

### 5.3 Test Commands

```bash
# Alle Completion-Tests ausführen
cargo test sql_completion --lib -- --nocapture

# Nur einen spezifischen Test
cargo test sql_completion::test_select_star --lib

# Mit Details
cargo test sql_completion --lib -- --nocapture --test-threads=1
```

### 5.4 Test Runner Script

```bash
#!/bin/bash
# scripts/test-sql-completion.sh

echo "═══════════════════════════════════════════════════════════════"
echo "           SQL Completion Test Suite"
echo "═══════════════════════════════════════════════════════════════"

# Unit Tests
echo ""
echo "▶ Running Unit Tests..."
cargo test sql_completion --lib 2>&1 | tee /tmp/completion-tests.log

# Count results
PASSED=$(grep -c "test .* ok" /tmp/completion-tests.log || echo "0")
FAILED=$(grep -c "test .* FAILED" /tmp/completion-tests.log || echo "0")

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "Results: ✅ $PASSED passed, ❌ $FAILED failed"
echo "═══════════════════════════════════════════════════════════════"

if [ "$FAILED" -gt 0 ]; then
    echo ""
    echo "Failed tests:"
    grep "FAILED" /tmp/completion-tests.log
    exit 1
fi
```

### 5.5 Integration Test (mit echter DB)

```rust
// tests/integration/sql_completion_integration.rs

#[tokio::test]
async fn test_completion_with_real_schema() {
    let db = setup_test_db().await;
    
    // Create test tables
    db.execute("CREATE TABLE users (id INT, name TEXT, email TEXT)").await;
    db.execute("CREATE TABLE tickets (id INT, user_id INT, title TEXT)").await;
    
    let engine = SqlCompletionEngine::new_with_db(&db).await;
    
    // Test that real table names appear
    let suggestions = engine.complete("SELECT * FROM ", 14);
    assert!(suggestions.contains(&"users".to_string()));
    assert!(suggestions.contains(&"tickets".to_string()));
    
    // Test that real column names appear
    let suggestions = engine.complete("SELECT * FROM users WHERE ", 26);
    assert!(suggestions.contains(&"id".to_string()));
    assert!(suggestions.contains(&"name".to_string()));
    assert!(suggestions.contains(&"email".to_string()));
}
```

### 5.6 Snapshot Testing (Optional)

```rust
// Für komplexe Outputs: Snapshot-Tests
#[test]
fn test_completion_snapshots() {
    let engine = SqlCompletionEngine::new_test_default();
    
    insta::assert_debug_snapshot!(
        "select_star",
        engine.complete("SELECT * ", 9)
    );
    
    insta::assert_debug_snapshot!(
        "insert_into",
        engine.complete("INSERT INTO ", 12)
    );
}
```

---

## Checkliste: Selbst-Validierung

Vor dem "Fertig"-Melden diese Checkliste durchgehen:

### Funktionalität
- [ ] `cargo test sql_completion` = 100% passed
- [ ] Leere Eingabe + TAB = Statement keywords
- [ ] `SELECT *` + TAB = NUR "FROM"
- [ ] `INSERT` + TAB = NUR "INTO"
- [ ] `UPDATE users` + TAB = NUR "SET"
- [ ] `DELETE` + TAB = NUR "FROM"
- [ ] Tabellen erscheinen nach FROM/JOIN/INTO
- [ ] Spalten erscheinen nach WHERE/SET/ORDER BY
- [ ] Aliases funktionieren (`u.` → columns of users)
- [ ] Case-insensitive (`select` = `SELECT`)
- [ ] Mehrere Leerzeichen = kein Problem
- [ ] Cycling funktioniert ohne Bugs

### Robustheit
- [ ] Keine Panics bei Edge Cases
- [ ] Leere Strings = handled
- [ ] Ungültiges SQL = graceful fallback
- [ ] Unicode in Strings = handled

### Performance
- [ ] < 10ms für Completion
- [ ] Kein Memory Leak bei vielen TABs

---

## Notizen

- **Kein vollständiger Parser nötig** - Unvollständiges SQL muss funktionieren
- **Kontext-Stack ist der Kern** - Trackt wo wir im SQL sind
- **Schema macht es intelligent** - Echte Tabellen/Spalten statt nur Keywords
- **Robust > Feature-rich** - Lieber weniger Features, aber zuverlässig
- **Tests sind PFLICHT** - Keine Änderung ohne grüne Tests

