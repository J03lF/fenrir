//! Box-styled table output.
//!
//! Renders tabular data with box borders.

use std::io::{self, Write};

use super::renderer::{visible_len, BoxRenderer};
use super::style::{BoxChars, COLOR_LABEL, COLOR_MUTED, COLOR_RESET, COLOR_VALUE, DEFAULT_CHARS};

/// Minimum box width
const MIN_WIDTH: usize = 40;
/// Padding inside box (left + right)
const BOX_PADDING: usize = 6;

/// A table with box borders
pub struct BoxTable {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    title: Option<String>,
    chars: BoxChars,
    show_count: bool,
    col_widths: Option<Vec<usize>>,
}

impl BoxTable {
    /// Create a new table with headers
    pub fn new(headers: Vec<String>) -> Self {
        Self {
            headers,
            rows: Vec::new(),
            title: None,
            chars: DEFAULT_CHARS,
            show_count: false,
            col_widths: None,
        }
    }

    /// Set the table title
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Show row count in title (e.g., "Services (12)")
    pub fn with_count(mut self) -> Self {
        self.show_count = true;
        self
    }

    /// Set the border style
    pub fn with_style(mut self, chars: BoxChars) -> Self {
        self.chars = chars;
        self
    }

    /// Set explicit column widths
    pub fn with_col_widths(mut self, widths: Vec<usize>) -> Self {
        self.col_widths = Some(widths);
        self
    }

    /// Add a row to the table
    pub fn add_row(&mut self, row: Vec<String>) {
        self.rows.push(row);
    }

    /// Check if table is empty
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Render the table
    pub fn render(&self, out: &mut dyn Write) -> io::Result<()> {
        // Calculate column widths first
        let col_widths = self.calculate_col_widths();

        // Calculate total content width
        let content_width = self.calculate_content_width(&col_widths);

        // Calculate dynamic box width
        let box_width = self.calculate_box_width(content_width);

        let renderer = BoxRenderer::new()
            .with_width(box_width)
            .with_style(self.chars);

        // Top border with optional title
        if let Some(title) = &self.title {
            let display_title = if self.show_count {
                format!("{} ({})", title, self.rows.len())
            } else {
                title.clone()
            };
            renderer.top_with_title(&display_title, out)?;
        } else {
            renderer.top(out)?;
        }

        // Empty row for spacing
        renderer.empty_row(out)?;

        // Headers
        let header_line = self.format_row(&self.headers, &col_widths, true);
        renderer.row(&header_line, out)?;

        // Header underline
        let underline = self.format_underline(&col_widths);
        renderer.row(&underline, out)?;

        // Data rows
        for row in &self.rows {
            let line = self.format_row(row, &col_widths, false);
            renderer.row(&line, out)?;
        }

        // Empty row for spacing
        renderer.empty_row(out)?;

        // Bottom border
        renderer.bottom(out)
    }

    /// Calculate optimal column widths based on content
    fn calculate_col_widths(&self) -> Vec<usize> {
        if let Some(widths) = &self.col_widths {
            return widths.clone();
        }

        let num_cols = self.headers.len();
        let mut widths: Vec<usize> = self.headers.iter().map(|h| visible_len(h)).collect();

        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < num_cols {
                    widths[i] = widths[i].max(visible_len(cell));
                }
            }
        }

        // Add minimum spacing per column
        for w in &mut widths {
            *w = (*w).max(4);
        }

        widths
    }

    /// Calculate total content width (all columns + spacing)
    fn calculate_content_width(&self, col_widths: &[usize]) -> usize {
        let total_col_width: usize = col_widths.iter().sum();
        let spacing = if col_widths.len() > 1 {
            (col_widths.len() - 1) * 2 // 2 spaces between columns
        } else {
            0
        };
        total_col_width + spacing + 2 // +2 for row prefix "  "
    }

    /// Calculate dynamic box width based on content
    fn calculate_box_width(&self, content_width: usize) -> usize {
        let needed = content_width + BOX_PADDING;
        needed.max(MIN_WIDTH)
    }

    /// Format a row with proper column alignment
    fn format_row(&self, cells: &[String], widths: &[usize], is_header: bool) -> String {
        let mut parts = Vec::new();
        let color = if is_header { COLOR_LABEL } else { COLOR_VALUE };

        for (i, cell) in cells.iter().enumerate() {
            let width = widths.get(i).copied().unwrap_or(10);
            let visible = visible_len(cell);
            let padding = width.saturating_sub(visible);

            parts.push(format!(
                "{}{}{}{reset}",
                color,
                cell,
                " ".repeat(padding),
                reset = COLOR_RESET
            ));
        }

        format!("  {}", parts.join("  "))
    }

    /// Format the header underline
    fn format_underline(&self, widths: &[usize]) -> String {
        let parts: Vec<String> = widths
            .iter()
            .map(|&w| format!("{}{}{}", COLOR_MUTED, "─".repeat(w), COLOR_RESET))
            .collect();
        format!("  {}", parts.join("  "))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Legacy compatibility
// ═══════════════════════════════════════════════════════════════════════════

/// Legacy Table API for backwards compatibility.
/// New code should use BoxTable directly.
pub struct Table {
    inner: BoxTable,
}

impl Table {
    pub fn new(headers: Vec<String>) -> Self {
        Self {
            inner: BoxTable::new(headers),
        }
    }

    pub fn with_spacing(self, _spacing: usize) -> Self {
        // Spacing is now handled automatically
        self
    }

    pub fn add_row(&mut self, row: Vec<String>) {
        self.inner.add_row(row);
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Render with legacy indent parameter (ignored, box handles padding)
    pub fn render(&self, out: &mut dyn Write, _indent: &str) -> io::Result<()> {
        self.inner.render(out)
    }
}
