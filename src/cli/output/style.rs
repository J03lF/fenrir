//! Style definitions for CLI output boxes.
//!
//! Contains border characters, colors, and symbols used throughout the output system.

/// Border characters for box rendering
#[derive(Debug, Clone, Copy)]
pub struct BoxChars {
    pub top_left: char,
    pub top_right: char,
    pub bottom_left: char,
    pub bottom_right: char,
    pub horizontal: char,
    pub vertical: char,
    pub left_tee: char,
    pub right_tee: char,
    pub top_tee: char,
    pub bottom_tee: char,
}

/// Rounded corners (default style)
pub const ROUNDED: BoxChars = BoxChars {
    top_left: '╭',
    top_right: '╮',
    bottom_left: '╰',
    bottom_right: '╯',
    horizontal: '─',
    vertical: '│',
    left_tee: '├',
    right_tee: '┤',
    top_tee: '┬',
    bottom_tee: '┴',
};

/// Sharp corners
pub const SHARP: BoxChars = BoxChars {
    top_left: '┌',
    top_right: '┐',
    bottom_left: '└',
    bottom_right: '┘',
    horizontal: '─',
    vertical: '│',
    left_tee: '├',
    right_tee: '┤',
    top_tee: '┬',
    bottom_tee: '┴',
};

/// Double line borders (for emphasis)
pub const DOUBLE: BoxChars = BoxChars {
    top_left: '╔',
    top_right: '╗',
    bottom_left: '╚',
    bottom_right: '╝',
    horizontal: '═',
    vertical: '║',
    left_tee: '╠',
    right_tee: '╣',
    top_tee: '╦',
    bottom_tee: '╩',
};

/// Default box style
pub const DEFAULT_CHARS: BoxChars = ROUNDED;

/// Default box width
pub const DEFAULT_WIDTH: usize = 64;

// ═══════════════════════════════════════════════════════════════════════════
// Colors (ANSI escape codes)
// ═══════════════════════════════════════════════════════════════════════════

pub const COLOR_RESET: &str = "\x1b[0m";
pub const COLOR_BOLD: &str = "\x1b[1m";

/// Border color (dark gray)
pub const COLOR_BORDER: &str = "\x1b[38;5;239m";

/// Title color (white)
pub const COLOR_TITLE: &str = "\x1b[38;5;255m";

/// Label color (gray)
pub const COLOR_LABEL: &str = "\x1b[38;5;245m";

/// Value color (light gray)
pub const COLOR_VALUE: &str = "\x1b[38;5;252m";

/// Accent color (teal)
pub const COLOR_ACCENT: &str = "\x1b[38;5;79m";

/// Success color (green)
pub const COLOR_SUCCESS: &str = "\x1b[38;5;114m";

/// Error color (red)
pub const COLOR_ERROR: &str = "\x1b[38;5;203m";

/// Warning color (orange)
pub const COLOR_WARNING: &str = "\x1b[38;5;214m";

/// Info color (blue)
pub const COLOR_INFO: &str = "\x1b[38;5;75m";

/// Muted color (dim gray)
pub const COLOR_MUTED: &str = "\x1b[38;5;242m";

// ═══════════════════════════════════════════════════════════════════════════
// Symbols
// ═══════════════════════════════════════════════════════════════════════════

pub const SYM_SUCCESS: &str = "✓";
pub const SYM_ERROR: &str = "✗";
pub const SYM_WARNING: &str = "⚠";
pub const SYM_INFO: &str = "ℹ";
pub const SYM_ACTIVE: &str = "●";
pub const SYM_INACTIVE: &str = "○";
pub const SYM_DEGRADED: &str = "◐";
pub const SYM_ARROW: &str = "→";
pub const SYM_BULLET: &str = "•";

// ═══════════════════════════════════════════════════════════════════════════
// Field styling
// ═══════════════════════════════════════════════════════════════════════════

/// Style for field values
#[derive(Debug, Clone, Copy, Default)]
pub enum FieldStyle {
    #[default]
    Normal,
    Accent,
    Success,
    Error,
    Warning,
    Muted,
}

impl FieldStyle {
    pub fn color(&self) -> &'static str {
        match self {
            FieldStyle::Normal => COLOR_VALUE,
            FieldStyle::Accent => COLOR_ACCENT,
            FieldStyle::Success => COLOR_SUCCESS,
            FieldStyle::Error => COLOR_ERROR,
            FieldStyle::Warning => COLOR_WARNING,
            FieldStyle::Muted => COLOR_MUTED,
        }
    }
}
