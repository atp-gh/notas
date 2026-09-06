//! GTK frontend adapters and frontend-neutral message conversion.

pub mod dialogs;
pub mod protocol;
pub mod settings;
pub mod status;
pub mod theme;

pub use protocol::{AppMsg, ViewMode};
