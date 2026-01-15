# TODO: Strukturierte Output-Boxen für Fenrir CLI

## Übersicht

Modernisierung der CLI-Ausgaben mit einheitlichen, professionellen Box-Layouts.
Alle Outputs nutzen ein zentrales Template-System mit runden Ecken (╭╮╰╯).

---

## Phase 1: Output-Framework (Core)

### 1.1 Neue Dateistruktur erstellen

```
src/cli/output/
├── mod.rs              # Re-exports
├── style.rs            # BoxChars, Farben, Konstanten
├── renderer.rs         # BoxRenderer (Kern-Logik)
├── table.rs            # BoxTable (ersetzt alte Table)
├── status_box.rs       # Key-Value Paare mit Sections
├── message_box.rs      # Error, Success, Info, Warning
└── card.rs             # Card-Layout für Detailansichten
```

### 1.2 Style-Definitionen (`style.rs`)

```rust
/// Border-Zeichen für Boxen
pub struct BoxChars {
    pub top_left: char,      // ╭
    pub top_right: char,     // ╮
    pub bottom_left: char,   // ╰
    pub bottom_right: char,  // ╯
    pub horizontal: char,    // ─
    pub vertical: char,      // │
    pub left_tee: char,      // ├
    pub right_tee: char,     // ┤
    pub top_tee: char,       // ┬
    pub bottom_tee: char,    // ┴
    pub cross: char,         // ┼
}

pub const ROUNDED: BoxChars = BoxChars { /* ... */ };
pub const SHARP: BoxChars = BoxChars { /* ... */ };
pub const DOUBLE: BoxChars = BoxChars { /* ... */ };

/// Farben (ANSI)
pub const COLOR_RESET: &str = "\x1b[0m";
pub const COLOR_BORDER: &str = "\x1b[38;5;239m";    // Dunkelgrau
pub const COLOR_TITLE: &str = "\x1b[38;5;255m";     // Weiß
pub const COLOR_LABEL: &str = "\x1b[38;5;245m";     // Grau
pub const COLOR_VALUE: &str = "\x1b[38;5;252m";     // Hellgrau
pub const COLOR_ACCENT: &str = "\x1b[38;5;79m";     // Teal
pub const COLOR_SUCCESS: &str = "\x1b[38;5;114m";   // Grün
pub const COLOR_ERROR: &str = "\x1b[38;5;203m";     // Rot
pub const COLOR_WARNING: &str = "\x1b[38;5;214m";   // Orange

/// Status-Symbole
pub const SYM_SUCCESS: &str = "✓";
pub const SYM_ERROR: &str = "✗";
pub const SYM_WARNING: &str = "⚠";
pub const SYM_INFO: &str = "ℹ";
pub const SYM_ACTIVE: &str = "●";
pub const SYM_INACTIVE: &str = "○";
pub const SYM_ARROW: &str = "→";
```

### 1.3 Box-Renderer (`renderer.rs`)

```rust
/// Kern-Renderer für alle Box-Typen
pub struct BoxRenderer {
    chars: BoxChars,
    width: usize,
    border_color: &'static str,
}

impl BoxRenderer {
    pub fn new() -> Self;
    pub fn with_width(self, width: usize) -> Self;
    pub fn with_style(self, chars: BoxChars) -> Self;
    
    /// Rendert: ╭─ Title ─────────────────────╮
    pub fn top_with_title(&self, title: &str, out: &mut dyn Write) -> io::Result<()>;
    
    /// Rendert: ╭─────────────────────────────╮
    pub fn top(&self, out: &mut dyn Write) -> io::Result<()>;
    
    /// Rendert: │ content                     │
    pub fn row(&self, content: &str, out: &mut dyn Write) -> io::Result<()>;
    
    /// Rendert: │                             │
    pub fn empty_row(&self, out: &mut dyn Write) -> io::Result<()>;
    
    /// Rendert: ├─────────────────────────────┤
    pub fn separator(&self, out: &mut dyn Write) -> io::Result<()>;
    
    /// Rendert: ╰─────────────────────────────╯
    pub fn bottom(&self, out: &mut dyn Write) -> io::Result<()>;
    
    /// Berechnet sichtbare String-Länge (ohne ANSI-Codes)
    fn visible_len(s: &str) -> usize;
    
    /// Padding für Zeile
    fn pad_content(&self, content: &str) -> String;
}
```

---

## Phase 2: High-Level Komponenten

### 2.1 BoxTable (`table.rs`)

Ersetzt die alte `Table` Klasse, behält API-Kompatibilität.

```rust
pub struct BoxTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    title: Option<String>,
    style: BoxChars,
    show_count: bool,
}

impl BoxTable {
    pub fn new(headers: Vec<String>) -> Self;
    pub fn with_title(self, title: impl Into<String>) -> Self;
    pub fn with_count(self) -> Self;  // Zeigt "(12)" im Title
    pub fn add_row(&mut self, row: Vec<String>);
    pub fn render(&self, out: &mut dyn Write) -> io::Result<()>;
}
```

**Output:**
```
╭─ Services (12) ───────────────────────────────────────────╮
│  ID              STATUS       PORT     UPTIME             │
│  ──────────────  ──────────   ──────   ────────           │
│  fenrir-api      ● Active     8080     2h 34m             │
│  auth-service    ● Active     8081     2h 34m             │
╰───────────────────────────────────────────────────────────╯
```

### 2.2 StatusBox (`status_box.rs`)

Für Detailansichten mit Key-Value Paaren.

```rust
pub struct StatusBox {
    title: String,
    sections: Vec<StatusSection>,
}

pub struct StatusSection {
    fields: Vec<(String, String, FieldStyle)>,
}

pub enum FieldStyle {
    Normal,
    Accent,
    Success,
    Error,
    Muted,
}

impl StatusBox {
    pub fn new(title: impl Into<String>) -> Self;
    pub fn field(self, label: &str, value: impl Display) -> Self;
    pub fn field_styled(self, label: &str, value: impl Display, style: FieldStyle) -> Self;
    pub fn section(self) -> Self;  // Fügt Separator ein
    pub fn render(&self, out: &mut dyn Write) -> io::Result<()>;
}
```

**Usage:**
```rust
StatusBox::new("fenrir-api")
    .field("Status", "● Active")
    .field_styled("Health", "✓ healthy", FieldStyle::Success)
    .field("Port", "8080")
    .field("Uptime", "2h 34m")
    .section()
    .field("Latency P50", "23ms")
    .field("Latency P95", "45ms")
    .field("Error Rate", "0.1%")
    .render(out)?;
```

**Output:**
```
╭─ fenrir-api ──────────────────────────────────────────────╮
│                                                           │
│  Status      ● Active           Health     ✓ healthy      │
│  Port        8080               Uptime     2h 34m         │
│                                                           │
├───────────────────────────────────────────────────────────┤
│                                                           │
│  Latency P50   23ms             Error Rate   0.1%         │
│  Latency P95   45ms                                       │
│                                                           │
╰───────────────────────────────────────────────────────────╯
```

### 2.3 MessageBox (`message_box.rs`)

Für Errors, Erfolge, Warnungen, Info.

```rust
pub enum MessageType {
    Error,
    Success,
    Warning,
    Info,
}

pub struct MessageBox {
    msg_type: MessageType,
    title: String,
    message: String,
    details: Vec<String>,
    suggestions: Vec<String>,
    code: Option<String>,
}

impl MessageBox {
    pub fn error(title: impl Into<String>) -> Self;
    pub fn success(title: impl Into<String>) -> Self;
    pub fn warning(title: impl Into<String>) -> Self;
    pub fn info(title: impl Into<String>) -> Self;
    
    pub fn message(self, msg: impl Into<String>) -> Self;
    pub fn detail(self, detail: impl Into<String>) -> Self;
    pub fn suggestion(self, suggestion: impl Into<String>) -> Self;
    pub fn code(self, code: impl Into<String>) -> Self;
    pub fn render(&self, out: &mut dyn Write) -> io::Result<()>;
}
```

**Usage:**
```rust
MessageBox::error("Module installation failed")
    .message("Registry 'https://registry.fenrir.dev' is not reachable")
    .code("REGISTRY_UNAVAILABLE")
    .suggestion("Check if offline_dirs is configured")
    .suggestion("Verify registry URL in config")
    .render(out)?;
```

**Output:**
```
╭─ Error ───────────────────────────────────────────────────╮
│                                                           │
│  ✗ Module installation failed                             │
│                                                           │
│  Registry 'https://registry.fenrir.dev' is not reachable  │
│                                                           │
│  Code: REGISTRY_UNAVAILABLE                               │
│                                                           │
├───────────────────────────────────────────────────────────┤
│  Suggestions:                                             │
│    → Check if offline_dirs is configured                  │
│    → Verify registry URL in config                        │
╰───────────────────────────────────────────────────────────╯
```

---

## Phase 3: Migration der Commands

### 3.1 Priorität 1 (High Impact)

| Datei | Änderung | Aufwand |
|-------|----------|---------|
| `status/handler.rs` | `show_service_status` → `StatusBox` | ~30 min |
| `status/handler.rs` | `show_db_runtime_status` → `StatusBox` | ~30 min |
| `services/list.rs` | `list_services` → `BoxTable` | ~20 min |
| `modules/output.rs` | `render_service_error` → `MessageBox::error` | ~20 min |
| `jobs/view.rs` | `render_jobs_table` → `BoxTable` | ~15 min |

### 3.2 Priorität 2 (Medium Impact)

| Datei | Änderung | Aufwand |
|-------|----------|---------|
| `modules/output.rs` | `render_manifest` → `StatusBox` | ~20 min |
| `modules/output.rs` | `render_distribution_plan` → `BoxTable` | ~15 min |
| `jobs/view.rs` | `render_job_status` → `StatusBox` | ~15 min |
| `audit/render.rs` | `render_event` → Custom oder `BoxTable` | ~30 min |
| `user/list.rs` | User-Liste → `BoxTable` | ~15 min |

### 3.3 Priorität 3 (Low Impact)

| Datei | Änderung | Aufwand |
|-------|----------|---------|
| `help/command.rs` | Help-Output anpassen | ~20 min |
| `show/handler.rs` | Config/Env-Anzeige → `StatusBox` | ~20 min |
| `backup/command.rs` | Backup-Status → `MessageBox::success` | ~10 min |
| `restore/command.rs` | Restore-Status → `MessageBox::success` | ~10 min |
| `db_shell/render.rs` | Query-Ergebnisse → `BoxTable` | ~30 min |

---

## Phase 4: Feinschliff

### 4.1 Konsistenz-Check

- [ ] Alle Tables nutzen `BoxTable`
- [ ] Alle Errors nutzen `MessageBox::error`
- [ ] Alle Success-Meldungen nutzen `MessageBox::success`
- [ ] Alle Detail-Ansichten nutzen `StatusBox`
- [ ] Farben sind einheitlich

### 4.2 SSH-Kompatibilität

- [ ] Testen über SSH-Verbindung
- [ ] Terminal-Breiten-Handling (dynamisch oder fix 80)
- [ ] Fallback für Terminals ohne Unicode-Support

### 4.3 Tests

- [ ] Unit-Tests für `BoxRenderer`
- [ ] Snapshot-Tests für Output-Komponenten
- [ ] Integration-Tests für Commands

---

## Beispiel-Migrationen

### Vorher (`status/handler.rs`):

```rust
fn show_service_status(deps: &CliDependencies, service_id: &str, out: &mut dyn Write) -> io::Result<()> {
    let snapshot = deps.services.registry().get(service_id)?;
    
    writeln!(out, "Service Detail")?;
    writeln!(out, "==============")?;
    writeln!(out, "ID: {}", snapshot.descriptor.id)?;
    writeln!(out, "Name: {}", snapshot.descriptor.name)?;
    writeln!(out, "Kind: {}", snapshot.descriptor.kind.as_str())?;
    writeln!(out, "Status: {}", snapshot.status.label())?;
    // ... 20+ weitere writeln!
}
```

### Nachher:

```rust
use crate::cli::output::{StatusBox, FieldStyle};

fn show_service_status(deps: &CliDependencies, service_id: &str, out: &mut dyn Write) -> io::Result<()> {
    let snapshot = deps.services.registry().get(service_id)?;
    let diagnostics = deps.services.service_diagnostics(service_id);
    
    StatusBox::new(&snapshot.descriptor.id)
        .field("Name", &snapshot.descriptor.name)
        .field("Kind", snapshot.descriptor.kind.as_str())
        .field_styled("Status", format_status(&snapshot.status), status_style(&snapshot.status))
        .field("Uptime", format_uptime(&snapshot.since))
        .section()
        .field("Health", format_health(diagnostics.as_ref()))
        .field("Latency P50", format_latency(diagnostics.and_then(|d| d.latency_p50_ms)))
        .field("Latency P95", format_latency(diagnostics.and_then(|d| d.latency_p95_ms)))
        .field("Error Rate", format_error_rate(diagnostics.and_then(|d| d.error_rate_pct)))
        .render(out)
}
```

---

## Zeitschätzung

| Phase | Aufwand |
|-------|---------|
| Phase 1: Core Framework | 6-8h |
| Phase 2: High-Level Komponenten | 4-6h |
| Phase 3: Migration (alle Commands) | 8-12h |
| Phase 4: Feinschliff & Tests | 4-6h |
| **Total** | **22-32h** |

---

## ⚠️ WICHTIG: Keine Platzhalter!

Bei der Implementierung und in den Beispielen **IMMER echte Daten** verwenden:

```rust
// ❌ FALSCH - Platzhalter
StatusBox::new("service-name")
    .field("Status", "status-value")
    .field("Port", "port-number")

// ✅ RICHTIG - Echte Fenrir-Daten
StatusBox::new(&snapshot.descriptor.id)
    .field("Status", snapshot.status.label())
    .field("Port", format!("{}", ingress.port))
```

**Warum?**
- Platzhalter führen zu Copy-Paste-Fehlern
- Echte Daten zeigen sofort ob die API passt
- Code ist direkt produktionsreif

**Gilt für:**
- Alle Code-Beispiele in diesem Dokument
- Alle Migrationen
- Alle Tests (echte Testdaten, keine "foo", "bar", "test123")

---

## Offene Entscheidungen

- [ ] Default Box-Breite: 60, 70, oder 80 Zeichen?
- [ ] Terminal-Breite dynamisch ermitteln?
- [ ] Farben deaktivierbar machen (--no-color Flag)?
- [ ] Unterschiedliche Styles pro Box-Typ? (z.B. Error = DOUBLE)

---

## Nächste Schritte

1. `src/cli/output/mod.rs` erstellen
2. `style.rs` mit allen Konstanten
3. `renderer.rs` als Kern-Logik
4. `table.rs` als erste nutzbare Komponente
5. Eine Beispiel-Migration (`list services`) zum Testen

