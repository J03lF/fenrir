// Terminal sequences
pub(super) const CLEAR_SCREEN: &str = "\x1B[2J\x1B[H";
pub(super) const COLOR_RESET: &str = "\x1b[0m";

// Professional monochrome palette with clean accent
pub(super) const COLOR_PRIMARY: &str = "\x1b[38;5;255m"; // Pure white
pub(super) const COLOR_ACCENT: &str = "\x1b[38;5;79m"; // Clean teal/cyan
pub(super) const COLOR_DIM: &str = "\x1b[38;5;245m"; // Mid gray
pub(super) const COLOR_PROMPT: &str = "\x1b[38;5;79m"; // Teal for prompt
pub(super) const COLOR_BRAND: &str = "\x1b[38;5;255m"; // White for brand
pub(super) const COLOR_SUBTLE: &str = "\x1b[38;5;239m"; // Dark line
pub(super) const COLOR_LABEL: &str = "\x1b[38;5;245m"; // Label gray
pub(super) const COLOR_VALUE: &str = "\x1b[38;5;252m"; // Value white
pub(super) const BOLD: &str = "\x1b[1m";
