//! GUI-neutral application layer.
//!
//! Owns the application protocol and orchestration types that sit between
//! a frontend (GTK or otherwise) and the data core: frontend intents
//! ([`AppCommand`]), service results ([`DbEvent`]), settings
//! ([`config::Settings`]), navigation ([`ViewId`]/[`ViewMode`]) and pure
//! editor state transitions. This layer may depend on [`crate::core`] but
//! embeds no SQL or remote-protocol details, and contains no GTK types.

#![deny(missing_docs)]

pub mod commands;
pub mod config;
pub mod events;
pub mod navigation;
pub mod state;

pub use commands::{AppCommand, DbCommand, EditorMode};
pub use events::DbEvent;
pub use navigation::{ViewId, ViewMode};

/// Compatibility alias for the database command name used by the GTK worker.
pub type DbMsg = DbCommand;

/// Compatibility alias for the frontend message name used by GTK widgets.
pub type AppMsg = AppCommand;
