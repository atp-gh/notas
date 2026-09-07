//! GUI/platform-free data core.
//!
//! This module intentionally contains no GUI types. Storage, search, notes,
//! and synchronization are sibling services; frontends translate toolkit
//! events into [`AppCommand`] values and render [`DbEvent`] results.

#![deny(missing_docs)]

pub mod commands;
pub mod config;
pub mod error;
pub mod events;
pub mod markdown;
pub mod model;
pub mod navigation;
pub mod repository;
pub mod state;
pub mod sync;

pub use commands::{AppCommand, DbCommand};
pub use error::{Error, Result};
pub use events::DbEvent;
pub use model::{Note, NoteId, Notebook, NotebookId, SearchHit, Tag, TagCount, TagId};
pub use navigation::{ViewId, ViewMode};

/// Compatibility alias for the database command name used by the GTK worker.
pub type DbMsg = DbCommand;

/// Compatibility alias for the frontend message name used by GTK widgets.
pub type AppMsg = AppCommand;
