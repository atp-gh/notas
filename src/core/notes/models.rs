//! Data models shared across the application core.
//!
//! Every struct mirrors one row of the SQLite schema (see [`crate::core::storage::schema`])
//! and is deserialized with sqlx's `FromRow`.

use sqlx::FromRow;

/// A notebook: a named folder that can nest other notebooks (via
/// [`Self::parent_id`]) and contains notes.
#[derive(Debug, Clone, FromRow)]
pub struct Notebook {
    /// Unique identifier.
    pub id: i64,
    /// Id of the parent notebook, if this notebook is nested.
    pub parent_id: Option<i64>,
    /// Display name.
    pub name: String,
    /// Creation time, as stored by SQLite (`YYYY-MM-DD HH:MM:SS`, UTC).
    pub created_at: String,
    /// Last modification time, in the same format as `created_at`.
    pub updated_at: String,
}

/// A note: a title plus Markdown content, with lifecycle state.
#[derive(Debug, Clone, FromRow)]
pub struct Note {
    /// Unique identifier.
    pub id: i64,
    /// Owning notebook, or `None` for unfiled notes.
    pub notebook_id: Option<i64>,
    /// Display title.
    pub title: String,
    /// Markdown source.
    pub content: String,
    /// Whether the note sits in the trash (hidden from normal lists).
    pub is_trashed: bool,
    /// Creation time, as stored by SQLite (`YYYY-MM-DD HH:MM:SS`, UTC).
    pub created_at: String,
    /// Last modification time, in the same format as `created_at`.
    pub updated_at: String,
}

/// A tag: a reusable, unique label applied to notes.
#[derive(Debug, Clone, FromRow)]
pub struct Tag {
    /// Unique identifier.
    pub id: i64,
    /// Display name (unique across tags).
    pub name: String,
}

/// One full-text search result: the note id, title and a highlighted snippet.
#[derive(Debug, Clone, FromRow)]
pub struct SearchHit {
    /// Id of the matching note.
    pub id: i64,
    /// Title of the matching note.
    pub title: String,
    /// Context snippet around the first match, with match markers (`⟪…⟫`).
    pub snippet: String,
}

/// A tag plus the number of notes currently carrying it.
#[derive(Debug, Clone, FromRow)]
pub struct TagCount {
    /// Unique identifier.
    pub id: i64,
    /// Display name (unique across tags).
    pub name: String,
    /// Number of notes using this tag.
    pub note_count: i64,
}
