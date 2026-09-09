//! Database-facing data models and typed row identifiers.
//!
//! Every struct mirrors one row of the SQLite schema (see
//! [`crate::core::database`]) and deserializes with sqlx's `FromRow`. Row
//! identifiers are small newtypes ([`NoteId`], [`NotebookId`], [`TagId`]) so
//! note, notebook, and tag ids cannot be mixed up at call sites; the inner
//! `i64` stays public for boundary conversions (e.g. GTK tree models store
//! raw integers).

use sqlx::FromRow;

/// Stable identifier of a note row (`notes.id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, sqlx::Type)]
#[sqlx(transparent)]
pub struct NoteId(
    /// Raw SQLite rowid.
    pub i64,
);

impl std::fmt::Display for NoteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Stable identifier of a notebook row (`notebooks.id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, sqlx::Type)]
#[sqlx(transparent)]
pub struct NotebookId(
    /// Raw SQLite rowid.
    pub i64,
);

impl std::fmt::Display for NotebookId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Stable identifier of a tag row (`tags.id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, sqlx::Type)]
#[sqlx(transparent)]
pub struct TagId(
    /// Raw SQLite rowid.
    pub i64,
);

impl std::fmt::Display for TagId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A notebook: a named folder that can nest other notebooks (via
/// [`Self::parent_id`]) and contains notes.
#[derive(Debug, Clone, FromRow)]
pub struct Notebook {
    /// Unique identifier.
    pub id: NotebookId,
    /// Id of the parent notebook, if this notebook is nested.
    pub parent_id: Option<NotebookId>,
    /// Display name.
    pub name: String,
    /// Whether the notebook sits in the trash (hidden from normal lists).
    pub is_trashed: bool,
    /// Creation time, as stored by SQLite (`YYYY-MM-DD HH:MM:SS`, UTC).
    pub created_at: String,
    /// Last modification time, in the same format as `created_at`.
    pub updated_at: String,
}

/// A note: a title plus Markdown content, with lifecycle state.
#[derive(Debug, Clone, FromRow)]
pub struct Note {
    /// Unique identifier.
    pub id: NoteId,
    /// Owning notebook, or `None` for unfiled notes.
    pub notebook_id: Option<NotebookId>,
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
    pub id: TagId,
    /// Display name (unique across tags).
    pub name: String,
}

/// One full-text search result: the note id, title and a highlighted snippet.
#[derive(Debug, Clone, FromRow)]
pub struct SearchHit {
    /// Id of the matching note.
    pub id: NoteId,
    /// Title of the matching note.
    pub title: String,
    /// Context snippet around the first match, with match markers (`⟪…⟫`).
    pub snippet: String,
}

/// A tag plus the number of notes currently carrying it.
#[derive(Debug, Clone, FromRow)]
pub struct TagCount {
    /// Unique identifier.
    pub id: TagId,
    /// Display name (unique across tags).
    pub name: String,
    /// Number of notes using this tag.
    pub note_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_display_as_their_raw_number() {
        assert_eq!(NoteId(7).to_string(), "7");
        assert_eq!(NotebookId(0).to_string(), "0");
        assert_eq!(TagId(12).to_string(), "12");
    }
}
