//! Data models shared across the application core.
//!
//! Compatibility shim: the canonical models live in
//! [`crate::core::model`]; this module re-exports them while GTK callers
//! migrate.

pub use crate::core::model::{Note, NoteId, Notebook, NotebookId, SearchHit, Tag, TagCount, TagId};
