//! Repository layer: every database operation in one place.
//!
//! Compatibility shim: the canonical implementation is the
//! [`Repository`](crate::core::repository::Repository) facade in the data
//! core; these free functions construct a `Repository` around the pool and
//! delegate, so existing callers keep working during the migration.

use std::path::Path;

use sqlx::SqlitePool;

use crate::core::error::Result;
use crate::core::repository::Repository;
use crate::domain_notes::{Note, Notebook, SearchHit, Tag, TagCount};
use crate::sync::{LocalNote, Sidecar};

fn repo(pool: &SqlitePool) -> Repository {
    Repository::new(pool.clone())
}

// ---------------------------------------------------------------------------
// Notebooks
// ---------------------------------------------------------------------------

/// Insert a notebook and return the created row.
pub async fn create_notebook(
    pool: &SqlitePool,
    parent_id: Option<i64>,
    name: &str,
) -> Result<Notebook> {
    repo(pool)
        .create_notebook(parent_id.map(crate::core::model::NotebookId), name)
        .await
}

/// Rename a notebook and bump its `updated_at`.
pub async fn rename_notebook(pool: &SqlitePool, id: i64, name: &str) -> Result<()> {
    repo(pool)
        .rename_notebook(crate::core::model::NotebookId(id), name)
        .await
}

/// Delete a notebook. Nested notebooks cascade; its notes become unfiled
/// (`notebook_id` is set to NULL by the schema).
pub async fn delete_notebook(pool: &SqlitePool, id: i64) -> Result<()> {
    repo(pool)
        .delete_notebook(crate::core::model::NotebookId(id))
        .await
}

/// All notebooks, sorted by name.
pub async fn list_notebooks(pool: &SqlitePool) -> Result<Vec<Notebook>> {
    repo(pool).list_notebooks().await
}

// ---------------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------------

/// Insert a note (with the given title) and return the created row.
pub async fn create_note(pool: &SqlitePool, notebook_id: Option<i64>, title: &str) -> Result<Note> {
    repo(pool)
        .create_note(notebook_id.map(crate::core::model::NotebookId), title)
        .await
}

/// Fetch a single note by id, if it exists.
pub async fn get_note(pool: &SqlitePool, id: i64) -> Result<Option<Note>> {
    repo(pool).get_note(crate::core::model::NoteId(id)).await
}

/// Update title + content; the FTS index is kept in sync by trigger `notes_au`.
pub async fn update_note(pool: &SqlitePool, id: i64, title: &str, content: &str) -> Result<()> {
    repo(pool)
        .update_note(crate::core::model::NoteId(id), title, content)
        .await
}

/// Move a note to the trash.
pub async fn trash_note(pool: &SqlitePool, id: i64) -> Result<()> {
    repo(pool).trash_note(crate::core::model::NoteId(id)).await
}

/// Restore a trashed note.
pub async fn restore_note(pool: &SqlitePool, id: i64) -> Result<()> {
    repo(pool)
        .restore_note(crate::core::model::NoteId(id))
        .await
}

/// Physically delete a note (FTS row removed by trigger `notes_ad`).
///
/// If the note had a sync uuid, a tombstone is recorded so the deletion
/// propagates to other devices (they trash their copy) instead of the
/// note resurrecting from the remote store on the next sync.
pub async fn delete_note_forever(pool: &SqlitePool, id: i64) -> Result<()> {
    repo(pool)
        .delete_note_forever(crate::core::model::NoteId(id))
        .await
}

/// Non-trashed notes of one notebook, newest first.
pub async fn list_notes(pool: &SqlitePool, notebook_id: i64) -> Result<Vec<Note>> {
    repo(pool)
        .list_notes(crate::core::model::NotebookId(notebook_id))
        .await
}

/// All non-trashed notes across every notebook, newest first.
pub async fn list_all_notes(pool: &SqlitePool) -> Result<Vec<Note>> {
    repo(pool).list_all_notes().await
}

/// All non-trashed notes carrying a given tag, newest first.
pub async fn list_notes_by_tag(pool: &SqlitePool, tag_id: i64) -> Result<Vec<Note>> {
    repo(pool)
        .list_notes_by_tag(crate::core::model::TagId(tag_id))
        .await
}

/// Notes whose notebook was deleted (notebook_id is NULL) but not trashed.
pub async fn list_unfiled_notes(pool: &SqlitePool) -> Result<Vec<Note>> {
    repo(pool).list_unfiled_notes().await
}

/// Trashed notes, newest first.
pub async fn list_trashed(pool: &SqlitePool) -> Result<Vec<Note>> {
    repo(pool).list_trashed().await
}

// ---------------------------------------------------------------------------
// Search (FTS5)
// ---------------------------------------------------------------------------

/// Full-text search over title + content, newest first, capped at 200 hits.
pub async fn search(pool: &SqlitePool, query: &str) -> Result<Vec<SearchHit>> {
    repo(pool).search(query).await
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// All tags with the number of notes using each, sorted by name.
pub async fn list_tags(pool: &SqlitePool) -> Result<Vec<TagCount>> {
    repo(pool).list_tags().await
}

/// The tags attached to one note, sorted by name.
pub async fn get_note_tags(pool: &SqlitePool, note_id: i64) -> Result<Vec<Tag>> {
    repo(pool)
        .get_note_tags(crate::core::model::NoteId(note_id))
        .await
}

/// Replace the tag set of a note. Tags are created on demand.
pub async fn set_note_tags(pool: &SqlitePool, note_id: i64, names: &[String]) -> Result<()> {
    repo(pool)
        .set_note_tags(crate::core::model::NoteId(note_id), names)
        .await
}

/// Rename a tag. The `tags.name` UNIQUE constraint rejects colliding names.
pub async fn rename_tag(pool: &SqlitePool, id: i64, name: &str) -> Result<()> {
    repo(pool)
        .rename_tag(crate::core::model::TagId(id), name)
        .await
}

/// Delete a tag; its `note_tags` links are removed by CASCADE.
pub async fn delete_tag(pool: &SqlitePool, id: i64) -> Result<()> {
    repo(pool).delete_tag(crate::core::model::TagId(id)).await
}

// ---------------------------------------------------------------------------
// Export & backup
// ---------------------------------------------------------------------------

/// Export every non-trashed note as `.md` files, grouped into notebook
/// subdirectories. Returns the number of notes written.
pub async fn export_markdown(pool: &SqlitePool, out_dir: &Path) -> Result<usize> {
    repo(pool).export_markdown(out_dir).await
}

/// Create a consistent snapshot of the database at `dest` using
/// `VACUUM INTO` (SQLite >= 3.27).
pub async fn backup(pool: &SqlitePool, dest: &Path) -> Result<()> {
    repo(pool).backup(dest).await
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

/// Assign a uuid to every note that does not have one yet. Idempotent.
pub async fn ensure_note_uuids(pool: &SqlitePool) -> Result<usize> {
    repo(pool).ensure_note_uuids().await
}

/// Build the full local index the planner needs.
pub async fn sync_local_index(pool: &SqlitePool) -> Result<Vec<LocalNote>> {
    repo(pool).sync_local_index().await
}

/// Apply a remote note locally in one transaction.
pub async fn apply_remote_note(pool: &SqlitePool, sidecar: &Sidecar, content: &str) -> Result<()> {
    repo(pool).apply_remote_note(sidecar, content).await
}

/// Create a local conflict copy of a remote note.
pub async fn create_conflict_copy(
    pool: &SqlitePool,
    sidecar: &Sidecar,
    content: &str,
) -> Result<()> {
    repo(pool).create_conflict_copy(sidecar, content).await
}

/// Move a note to the trash without bumping its `updated_at`.
pub async fn trash_note_by_uuid_no_bump(pool: &SqlitePool, uuid: &str) -> Result<()> {
    repo(pool).trash_note_by_uuid_no_bump(uuid).await
}

/// Every locally recorded tombstone.
pub async fn list_tombstones(pool: &SqlitePool) -> Result<Vec<(String, String)>> {
    repo(pool).list_tombstones().await
}
