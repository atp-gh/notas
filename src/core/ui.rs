//! Frontend-neutral navigation types.

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

/// Current navigation mode shared by frontends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewMode {
    /// All non-deleted notes.
    All,
    /// Notes without a notebook.
    Unfiled,
    /// Notes in the trash.
    Trash,
    /// Notes belonging to a notebook.
    Notebook(i64),
    /// Notes carrying a tag.
    Tag(i64),
    /// Full-text search results.
    Search(String),
}
