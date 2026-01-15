//! Message boxes for errors, success, warnings, and info.
//!
//! Provides consistent styling for feedback messages.

use std::io::{self, Write};

use super::renderer::{visible_len, BoxRenderer};
use super::style::{
    BoxChars, COLOR_ERROR, COLOR_INFO, COLOR_LABEL, COLOR_MUTED, COLOR_RESET, COLOR_SUCCESS,
    COLOR_WARNING, DEFAULT_CHARS, DOUBLE, SYM_ARROW, SYM_ERROR, SYM_INFO, SYM_SUCCESS, SYM_WARNING,
};

/// Minimum box width
const MIN_WIDTH: usize = 40;
/// Maximum box width
const MAX_WIDTH: usize = 80;
/// Padding inside box
const BOX_PADDING: usize = 6;

/// Type of message
#[derive(Debug, Clone, Copy)]
pub enum MessageType {
    Error,
    Success,
    Warning,
    Info,
}

impl MessageType {
    fn color(&self) -> &'static str {
        match self {
            MessageType::Error => COLOR_ERROR,
            MessageType::Success => COLOR_SUCCESS,
            MessageType::Warning => COLOR_WARNING,
            MessageType::Info => COLOR_INFO,
        }
    }

    fn symbol(&self) -> &'static str {
        match self {
            MessageType::Error => SYM_ERROR,
            MessageType::Success => SYM_SUCCESS,
            MessageType::Warning => SYM_WARNING,
            MessageType::Info => SYM_INFO,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            MessageType::Error => "Error",
            MessageType::Success => "Success",
            MessageType::Warning => "Warning",
            MessageType::Info => "Info",
        }
    }

    fn chars(&self) -> BoxChars {
        match self {
            MessageType::Error => DOUBLE,
            _ => DEFAULT_CHARS,
        }
    }
}

/// A message box for feedback
pub struct MessageBox {
    msg_type: MessageType,
    title: String,
    message: Option<String>,
    details: Vec<String>,
    suggestions: Vec<String>,
    code: Option<String>,
}

impl MessageBox {
    /// Create an error message box
    pub fn error(title: impl Into<String>) -> Self {
        Self::new(MessageType::Error, title)
    }

    /// Create a success message box
    pub fn success(title: impl Into<String>) -> Self {
        Self::new(MessageType::Success, title)
    }

    /// Create a warning message box
    pub fn warning(title: impl Into<String>) -> Self {
        Self::new(MessageType::Warning, title)
    }

    /// Create an info message box
    pub fn info(title: impl Into<String>) -> Self {
        Self::new(MessageType::Info, title)
    }

    fn new(msg_type: MessageType, title: impl Into<String>) -> Self {
        Self {
            msg_type,
            title: title.into(),
            message: None,
            details: Vec::new(),
            suggestions: Vec::new(),
            code: None,
        }
    }

    /// Add a detailed message
    pub fn message(mut self, msg: impl Into<String>) -> Self {
        self.message = Some(msg.into());
        self
    }

    /// Add a detail line
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }

    /// Add a suggestion
    pub fn suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestions.push(suggestion.into());
        self
    }

    /// Set an error code
    pub fn code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Calculate dynamic width based on content
    fn calculate_width(&self) -> usize {
        let mut max_content_width: usize = 0;

        // Check title width (with symbol)
        max_content_width = max_content_width.max(visible_len(&self.title) + 4);

        // Check message width
        if let Some(msg) = &self.message {
            max_content_width = max_content_width.max(visible_len(msg));
        }

        // Check details
        for detail in &self.details {
            max_content_width = max_content_width.max(visible_len(detail) + 4);
        }

        // Check suggestions
        for suggestion in &self.suggestions {
            max_content_width = max_content_width.max(visible_len(suggestion) + 6);
        }

        // Check code
        if let Some(code) = &self.code {
            max_content_width = max_content_width.max(visible_len(code) + 10);
        }

        let needed = max_content_width + BOX_PADDING;
        needed.clamp(MIN_WIDTH, MAX_WIDTH)
    }

    /// Render the message box
    pub fn render(self, out: &mut dyn Write) -> io::Result<()> {
        let chars = self.msg_type.chars();
        let width = self.calculate_width();

        let renderer = BoxRenderer::new().with_width(width).with_style(chars);

        let color = self.msg_type.color();
        let symbol = self.msg_type.symbol();
        let label = self.msg_type.label();

        // Top border with label
        renderer.top_with_title(label, out)?;

        // Empty row
        renderer.empty_row(out)?;

        // Title with symbol
        let title_line = format!(
            "  {color}{symbol}{reset} {title}",
            color = color,
            symbol = symbol,
            reset = COLOR_RESET,
            title = self.title,
        );
        renderer.row(&title_line, out)?;

        // Empty row after title
        renderer.empty_row(out)?;

        // Message if present
        if let Some(msg) = &self.message {
            // Word wrap the message
            let lines = wrap_text(msg, renderer.inner_width() - 2);
            for line in lines {
                let msg_line = format!("  {}", line);
                renderer.row(&msg_line, out)?;
            }
            renderer.empty_row(out)?;
        }

        // Details
        for detail in &self.details {
            let detail_line = format!(
                "  {muted}{bullet}{reset} {detail}",
                muted = COLOR_MUTED,
                bullet = "•",
                reset = COLOR_RESET,
                detail = detail,
            );
            renderer.row(&detail_line, out)?;
        }

        // Code if present
        if let Some(code) = &self.code {
            if self.message.is_some() || !self.details.is_empty() {
                renderer.empty_row(out)?;
            }
            let code_line = format!(
                "  {label}Code:{reset} {code}",
                label = COLOR_LABEL,
                reset = COLOR_RESET,
                code = code,
            );
            renderer.row(&code_line, out)?;
            renderer.empty_row(out)?;
        }

        // Suggestions
        if !self.suggestions.is_empty() {
            renderer.separator(out)?;

            let header = format!(
                "  {label}Suggestions:{reset}",
                label = COLOR_LABEL,
                reset = COLOR_RESET,
            );
            renderer.row(&header, out)?;

            for suggestion in &self.suggestions {
                let sug_line = format!(
                    "    {muted}{arrow}{reset} {suggestion}",
                    muted = COLOR_MUTED,
                    arrow = SYM_ARROW,
                    reset = COLOR_RESET,
                    suggestion = suggestion,
                );
                renderer.row(&sug_line, out)?;
            }
        }

        // Bottom border
        renderer.bottom(out)
    }
}

/// Simple word wrapping
fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_len = word.len();

        if current_width + word_len + 1 > max_width && !current_line.is_empty() {
            lines.push(current_line);
            current_line = String::new();
            current_width = 0;
        }

        if !current_line.is_empty() {
            current_line.push(' ');
            current_width += 1;
        }
        current_line.push_str(word);
        current_width += word_len;
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}
