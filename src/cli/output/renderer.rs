//! Core box rendering logic.
//!
//! Provides low-level primitives for drawing box borders and content rows.

use std::io::{self, Write};

use super::style::{
    BoxChars, COLOR_BORDER, COLOR_RESET, COLOR_TITLE, DEFAULT_CHARS, DEFAULT_WIDTH,
};

/// Core renderer for box-based output
pub struct BoxRenderer {
    chars: BoxChars,
    width: usize,
}

impl Default for BoxRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl BoxRenderer {
    /// Create a new renderer with default settings
    pub fn new() -> Self {
        Self {
            chars: DEFAULT_CHARS,
            width: DEFAULT_WIDTH,
        }
    }

    /// Set the box width
    pub fn with_width(mut self, width: usize) -> Self {
        self.width = width;
        self
    }

    /// Set the border style
    pub fn with_style(mut self, chars: BoxChars) -> Self {
        self.chars = chars;
        self
    }

    /// Get the inner content width (excluding borders and padding)
    pub fn inner_width(&self) -> usize {
        self.width.saturating_sub(4) // 2 for borders, 2 for padding
    }

    /// Render top border with title: ╭─ Title ─────────────────╮
    pub fn top_with_title(&self, title: &str, out: &mut dyn Write) -> io::Result<()> {
        let c = &self.chars;
        let title_display = format!(" {} ", title);
        let title_len = visible_len(&title_display);
        // width = top_left(1) + horizontal(1) + title + remaining + top_right(1)
        let remaining = self.width.saturating_sub(3 + title_len);

        write!(
            out,
            "{border}{tl}{h}{reset}{title_color}{title}{reset}{border}",
            border = COLOR_BORDER,
            tl = c.top_left,
            h = c.horizontal,
            reset = COLOR_RESET,
            title_color = COLOR_TITLE,
            title = title_display,
        )?;

        for _ in 0..remaining {
            write!(out, "{}{}{}", COLOR_BORDER, c.horizontal, COLOR_RESET)?;
        }

        writeln!(out, "{}{}{}", COLOR_BORDER, c.top_right, COLOR_RESET)
    }

    /// Render top border without title: ╭─────────────────────────╮
    pub fn top(&self, out: &mut dyn Write) -> io::Result<()> {
        let c = &self.chars;
        write!(out, "{}{}", COLOR_BORDER, c.top_left)?;
        for _ in 0..(self.width - 2) {
            write!(out, "{}", c.horizontal)?;
        }
        writeln!(out, "{}{}", c.top_right, COLOR_RESET)
    }

    /// Render a content row: │ content                     │
    pub fn row(&self, content: &str, out: &mut dyn Write) -> io::Result<()> {
        let c = &self.chars;
        let content_visible_len = visible_len(content);
        let inner = self.inner_width();

        // Use max of inner_width and content length to avoid truncation
        let actual_width = inner.max(content_visible_len);
        let padding = actual_width.saturating_sub(content_visible_len);

        write!(
            out,
            "{border}{v}{reset} {content}",
            border = COLOR_BORDER,
            v = c.vertical,
            reset = COLOR_RESET,
            content = content,
        )?;

        for _ in 0..padding {
            write!(out, " ")?;
        }

        writeln!(out, " {}{}{}", COLOR_BORDER, c.vertical, COLOR_RESET)
    }

    /// Render an empty row: │                             │
    pub fn empty_row(&self, out: &mut dyn Write) -> io::Result<()> {
        let c = &self.chars;
        write!(out, "{}{}", COLOR_BORDER, c.vertical)?;
        for _ in 0..(self.width - 2) {
            write!(out, " ")?;
        }
        writeln!(out, "{}{}", c.vertical, COLOR_RESET)
    }

    /// Render separator: ├─────────────────────────────┤
    pub fn separator(&self, out: &mut dyn Write) -> io::Result<()> {
        let c = &self.chars;
        write!(out, "{}{}", COLOR_BORDER, c.left_tee)?;
        for _ in 0..(self.width - 2) {
            write!(out, "{}", c.horizontal)?;
        }
        writeln!(out, "{}{}", c.right_tee, COLOR_RESET)
    }

    /// Render separator with title: ├─ Title ────────────────────┤
    pub fn separator_with_title(&self, title: &str, out: &mut dyn Write) -> io::Result<()> {
        let c = &self.chars;
        let title_display = format!(" {} ", title);
        let title_len = visible_len(&title_display);
        let remaining = self.width.saturating_sub(3 + title_len);

        write!(
            out,
            "{border}{lt}{h}{reset}{title_color}{title}{reset}{border}",
            border = COLOR_BORDER,
            lt = c.left_tee,
            h = c.horizontal,
            reset = COLOR_RESET,
            title_color = COLOR_TITLE,
            title = title_display,
        )?;

        for _ in 0..remaining {
            write!(out, "{}", c.horizontal)?;
        }

        writeln!(out, "{}{}", c.right_tee, COLOR_RESET)
    }

    /// Render bottom border: ╰─────────────────────────────╯
    pub fn bottom(&self, out: &mut dyn Write) -> io::Result<()> {
        let c = &self.chars;
        write!(out, "{}{}", COLOR_BORDER, c.bottom_left)?;
        for _ in 0..(self.width - 2) {
            write!(out, "{}", c.horizontal)?;
        }
        writeln!(out, "{}{}", c.bottom_right, COLOR_RESET)
    }
}

/// Calculate visible string length (excluding ANSI escape codes)
pub fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;

    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else {
            len += 1;
        }
    }

    len
}
