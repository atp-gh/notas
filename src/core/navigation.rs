//! Navigation state shared by all frontends.

use crate::core::model::{NotebookId, TagId};

/// Built-in top-level note views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewId {
    /// All non-deleted notes.
    All,
    /// Notes without a notebook.
    Unfiled,
    /// Notes in the trash.
    Trash,
}

/// Current navigation mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewMode {
    /// All non-deleted notes.
    All,
    /// Notes without a notebook.
    Unfiled,
    /// Notes in the trash.
    Trash,
    /// Notes belonging to a notebook.
    Notebook(NotebookId),
    /// Notes carrying a tag.
    Tag(TagId),
    /// Full-text search results.
    Search(String),
}
