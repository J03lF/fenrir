//! Structured output components for Fenrir CLI.
//!
//! Provides consistent, professional box-based output styling.
//!
//! # Components
//!
//! - [`BoxTable`] - Tabular data with box borders
//! - [`StatusBox`] - Key-value detail views
//! - [`MessageBox`] - Error, success, warning, and info messages
//!
//! # Example
//!
//! ```ignore
//! use crate::cli::output::{BoxTable, StatusBox, MessageBox, FieldStyle};
//!
//! // Table with title
//! let mut table = BoxTable::new(vec!["ID".into(), "STATUS".into()])
//!     .with_title("Services")
//!     .with_count();
//! table.add_row(vec!["fenrir-api".into(), "● Active".into()]);
//! table.render(&mut stdout)?;
//!
//! // Status detail view
//! StatusBox::new("fenrir-api")
//!     .field("Status", "● Active")
//!     .field_styled("Health", "✓ healthy", FieldStyle::Success)
//!     .section()
//!     .field("Port", "8080")
//!     .render(&mut stdout)?;
//!
//! // Error message
//! MessageBox::error("Installation failed")
//!     .message("Registry is not reachable")
//!     .code("REGISTRY_UNAVAILABLE")
//!     .suggestion("Check network connection")
//!     .render(&mut stdout)?;
//! ```

mod message_box;
mod renderer;
mod status_box;
pub mod style;
mod table;

// Re-exports
pub use message_box::{MessageBox, MessageType};
pub use renderer::{visible_len, BoxRenderer};
pub use status_box::StatusBox;
pub use style::{BoxChars, FieldStyle, DOUBLE, ROUNDED, SHARP};
pub use table::{BoxTable, Table};

// Convenience re-exports of common symbols
pub use style::{
    SYM_ACTIVE, SYM_ARROW, SYM_DEGRADED, SYM_ERROR, SYM_INACTIVE, SYM_INFO, SYM_SUCCESS,
    SYM_WARNING,
};
