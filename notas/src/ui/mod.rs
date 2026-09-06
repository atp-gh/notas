//! GTK frontend adapters and frontend-neutral message conversion.

pub mod dialogs;
pub mod protocol;
pub mod settings;
pub mod status;

pub use protocol::{AppMsg, ViewId, ViewMode};
