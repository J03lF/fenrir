//! Status box for key-value displays.
//!
//! Used for detail views like `status service`, `status db`, etc.

use std::fmt::Display;
use std::io::{self, Write};

use super::renderer::{visible_len, BoxRenderer};
use super::style::{BoxChars, FieldStyle, COLOR_LABEL, COLOR_RESET, DEFAULT_CHARS};

/// Minimum box width
const MIN_WIDTH: usize = 40;
/// Padding inside box (borders + margins)
const BOX_PADDING: usize = 6;
/// Default label width
const LABEL_WIDTH: usize = 14;
/// Max value length before forcing single column
const MAX_VALUE_FOR_TWO_COLS: usize = 25;

/// A field in a status box
struct Field {
    label: String,
    value: String,
    style: FieldStyle,
}

/// A section contains multiple fields
struct Section {
    title: Option<String>,
    fields: Vec<Field>,
}

/// Status box for displaying key-value pairs
pub struct StatusBox {
    title: String,
    sections: Vec<Section>,
    current_section: Section,
    chars: BoxChars,
    cols: usize,
}

impl StatusBox {
    /// Create a new status box with a title
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            sections: Vec::new(),
            current_section: Section {
                title: None,
                fields: Vec::new(),
            },
            chars: DEFAULT_CHARS,
            cols: 2, // Default to 2 columns
        }
    }

    /// Set the border style
    pub fn with_style(mut self, chars: BoxChars) -> Self {
        self.chars = chars;
        self
    }

    /// Set number of columns (1 or 2)
    pub fn with_cols(mut self, cols: usize) -> Self {
        self.cols = cols.clamp(1, 2);
        self
    }

    /// Add a field with default style
    pub fn field(self, label: &str, value: impl Display) -> Self {
        self.field_styled(label, value, FieldStyle::Normal)
    }

    /// Add a field with custom style
    pub fn field_styled(mut self, label: &str, value: impl Display, style: FieldStyle) -> Self {
        self.current_section.fields.push(Field {
            label: label.to_string(),
            value: value.to_string(),
            style,
        });
        self
    }

    /// Add a section separator
    pub fn section(mut self) -> Self {
        if !self.current_section.fields.is_empty() {
            self.sections.push(std::mem::replace(
                &mut self.current_section,
                Section {
                    title: None,
                    fields: Vec::new(),
                },
            ));
        }
        self
    }

    /// Add a section separator with a title
    pub fn section_titled(mut self, title: impl Into<String>) -> Self {
        if !self.current_section.fields.is_empty() {
            self.sections.push(std::mem::replace(
                &mut self.current_section,
                Section {
                    title: Some(title.into()),
                    fields: Vec::new(),
                },
            ));
        } else {
            self.current_section.title = Some(title.into());
        }
        self
    }

    /// Calculate dynamic width based on content
    fn calculate_width(&self) -> usize {
        let mut max_content_width: usize = 0;

        // Check title width
        max_content_width = max_content_width.max(visible_len(&self.title) + 4);

        // Check all fields
        let all_fields: Vec<&Field> = self
            .sections
            .iter()
            .flat_map(|s| s.fields.iter())
            .chain(self.current_section.fields.iter())
            .collect();

        if self.cols == 1 {
            // Single column: label + value
            for field in &all_fields {
                let line_width = LABEL_WIDTH + 4 + visible_len(&field.value);
                max_content_width = max_content_width.max(line_width);
            }
        } else {
            // Two columns: find widest pair
            for pair in all_fields.chunks(2) {
                let pair_width = match pair {
                    [f1, f2] => {
                        let left = LABEL_WIDTH + 4 + visible_len(&f1.value);
                        let right = LABEL_WIDTH + 4 + visible_len(&f2.value);
                        left + right + 2 // +2 for gap between columns
                    }
                    [f1] => LABEL_WIDTH + 4 + visible_len(&f1.value),
                    _ => 0,
                };
                max_content_width = max_content_width.max(pair_width);
            }
        }

        let needed = max_content_width + BOX_PADDING;
        needed.max(MIN_WIDTH)
    }

    /// Render the status box
    pub fn render(mut self, out: &mut dyn Write) -> io::Result<()> {
        // Flush current section
        if !self.current_section.fields.is_empty() {
            let section = std::mem::replace(
                &mut self.current_section,
                Section {
                    title: None,
                    fields: Vec::new(),
                },
            );
            self.sections.push(section);
        }

        // Check if any value is too long for two columns - auto-switch to single column
        let effective_cols = if self.cols > 1 {
            let has_long_value = self
                .sections
                .iter()
                .flat_map(|s| s.fields.iter())
                .any(|f| visible_len(&f.value) > MAX_VALUE_FOR_TWO_COLS);
            if has_long_value {
                1
            } else {
                self.cols
            }
        } else {
            self.cols
        };

        // Use effective_cols for width calculation
        let saved_cols = self.cols;
        self.cols = effective_cols;
        let width = self.calculate_width();
        self.cols = saved_cols;

        let renderer = BoxRenderer::new().with_width(width).with_style(self.chars);

        // Top border with title
        renderer.top_with_title(&self.title, out)?;

        let section_count = self.sections.len();
        let inner_width = renderer.inner_width();

        // Render each section
        for (i, section) in self.sections.iter().enumerate() {
            // Section title separator if present
            if let Some(title) = &section.title {
                renderer.separator_with_title(title, out)?;
            }

            // Empty row before content
            renderer.empty_row(out)?;

            // Render fields with effective_cols
            Self::render_fields(&section.fields, &renderer, out, effective_cols, inner_width)?;

            // Empty row after content
            renderer.empty_row(out)?;

            // Separator between sections (not after last, only if next section has no title)
            if i < section_count - 1 {
                let next_has_title = self
                    .sections
                    .get(i + 1)
                    .map(|s| s.title.is_some())
                    .unwrap_or(false);
                if !next_has_title {
                    renderer.separator(out)?;
                }
            }
        }

        // Bottom border
        renderer.bottom(out)
    }

    /// Render fields in a section (static to avoid borrow issues)
    fn render_fields(
        fields: &[Field],
        renderer: &BoxRenderer,
        out: &mut dyn Write,
        cols: usize,
        inner_width: usize,
    ) -> io::Result<()> {
        if cols == 1 {
            // Single column layout
            for field in fields {
                let line = Self::format_single_field(field, LABEL_WIDTH, inner_width);
                renderer.row(&line, out)?;
            }
        } else {
            // Two column layout
            let col_width = (inner_width - 4) / 2; // -4 for spacing between columns

            for pair in fields.chunks(2) {
                let line = match pair {
                    [f1, f2] => Self::format_field_pair(f1, f2, LABEL_WIDTH, col_width),
                    [f1] => Self::format_single_field(f1, LABEL_WIDTH, col_width),
                    _ => String::new(),
                };
                renderer.row(&line, out)?;
            }
        }

        Ok(())
    }

    /// Format a single field
    fn format_single_field(field: &Field, label_width: usize, _total_width: usize) -> String {
        let color = field.style.color();
        let label_padded = format!("{:width$}", field.label, width = label_width);

        format!(
            "  {label_color}{label}{reset}  {value_color}{value}{reset}",
            label_color = COLOR_LABEL,
            label = label_padded,
            reset = COLOR_RESET,
            value_color = color,
            value = field.value,
        )
    }

    /// Format two fields side by side
    fn format_field_pair(f1: &Field, f2: &Field, label_width: usize, col_width: usize) -> String {
        let color1 = f1.style.color();
        let color2 = f2.style.color();

        // Calculate max value width for first column
        let f1_label = format!("{:width$}", f1.label, width = label_width);
        let f1_value_max = col_width.saturating_sub(label_width + 4); // +4 for spacing

        // Truncate or pad first value to fixed width
        let f1_value_visible = visible_len(&f1.value);
        let (f1_display, f1_padding) = if f1_value_visible > f1_value_max {
            // Truncate with ellipsis
            let truncated = truncate_with_ellipsis(&f1.value, f1_value_max);
            (truncated, 0)
        } else {
            (f1.value.clone(), f1_value_max - f1_value_visible)
        };

        let f2_label = format!("{:width$}", f2.label, width = label_width);

        format!(
            "  {lc}{l1}{r}  {vc1}{v1}{r}{pad}  {lc}{l2}{r}  {vc2}{v2}{r}",
            lc = COLOR_LABEL,
            l1 = f1_label,
            r = COLOR_RESET,
            vc1 = color1,
            v1 = f1_display,
            pad = " ".repeat(f1_padding),
            l2 = f2_label,
            vc2 = color2,
            v2 = f2.value,
        )
    }
}

/// Truncate a string to max visible length with ellipsis
fn truncate_with_ellipsis(s: &str, max_len: usize) -> String {
    if max_len < 4 {
        return s.chars().take(max_len).collect();
    }
    let visible = visible_len(s);
    if visible <= max_len {
        return s.to_string();
    }
    // Take chars up to max_len - 1 and add "…"
    let mut result = String::new();
    for (count, ch) in s.chars().enumerate() {
        if count >= max_len - 1 {
            break;
        }
        result.push(ch);
    }
    result.push('…');
    result
}
