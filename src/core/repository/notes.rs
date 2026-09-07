//! Note rows: CRUD, lifecycle state, and note queries.

use sqlx::SqlitePool;

use crate::core::error::Result;
use crate::core::model::{Note, NoteId, NotebookId, TagId};

/// Insert a note (with the given title) and return the created row.
pub(crate) async fn create(
    pool: &SqlitePool,
    notebook_id: Option<NotebookId>,
    title: &str,
) -> Result<Note> {
    let row = sqlx::query_as::<_, Note>(
        "INSERT INTO notes (notebook_id, title) VALUES (?, ?) RETURNING \
         id, notebook_id, title, content, is_trashed, created_at, updated_at",
    )
    .bind(notebook_id)
    .bind(title)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Fetch a single note by id, if it exists.
pub(crate) async fn get(pool: &SqlitePool, id: NoteId) -> Result<Option<Note>> {
    let note = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(note)
}

/// Update title + content; the FTS index is kept in sync by trigger `notes_au`.
pub(crate) async fn update(
    pool: &SqlitePool,
    id: NoteId,
    title: &str,
    content: &str,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE notes SET title = ?, content = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(title)
    .bind(content)
    .bind(id)
    .execute(pool)
    .await?;
    ensure_affected(result.rows_affected(), || {
        crate::core::error::Error::NoteNotFound(id)
    })
}

/// Move a note to the trash.
pub(crate) async fn trash(pool: &SqlitePool, id: NoteId) -> Result<()> {
    let result =
        sqlx::query("UPDATE notes SET is_trashed = 1, updated_at = datetime('now') WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
    ensure_affected(result.rows_affected(), || {
        crate::core::error::Error::NoteNotFound(id)
    })
}

/// Restore a trashed note.
pub(crate) async fn restore(pool: &SqlitePool, id: NoteId) -> Result<()> {
    let result =
        sqlx::query("UPDATE notes SET is_trashed = 0, updated_at = datetime('now') WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
    ensure_affected(result.rows_affected(), || {
        crate::core::error::Error::NoteNotFound(id)
    })
}

/// Physically delete a note (FTS row removed by trigger `notes_ad`).
///
/// If the note had a sync uuid, a tombstone is recorded so the deletion
/// propagates to other devices (they trash their copy) instead of the
/// note resurrecting from the remote store on the next sync.
pub(crate) async fn delete_forever(pool: &SqlitePool, id: NoteId) -> Result<()> {
    let mut tx = pool.begin().await?;
    let uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM notes WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(crate::core::error::Error::NoteNotFound(id))?;
    let deleted = sqlx::query("DELETE FROM notes WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted > 0
        && let Some(uuid) = uuid
    {
        sqlx::query("INSERT OR IGNORE INTO sync_tombstones (uuid) VALUES (?)")
            .bind(uuid)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    ensure_affected(deleted, || crate::core::error::Error::NoteNotFound(id))
}

/// Non-trashed notes of one notebook, newest first.
pub(crate) async fn list_by_notebook(
    pool: &SqlitePool,
    notebook_id: NotebookId,
) -> Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE notebook_id = ? AND is_trashed = 0 \
         ORDER BY updated_at DESC",
    )
    .bind(notebook_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// All non-trashed notes across every notebook, newest first.
pub(crate) async fn list_all(pool: &SqlitePool) -> Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE is_trashed = 0 ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// All non-trashed notes carrying a given tag, newest first.
pub(crate) async fn list_by_tag(pool: &SqlitePool, tag_id: TagId) -> Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT n.id, n.notebook_id, n.title, n.content, n.is_trashed, n.created_at, n.updated_at \
         FROM notes n \
         JOIN note_tags nt ON nt.note_id = n.id \
         WHERE nt.tag_id = ? AND n.is_trashed = 0 \
         ORDER BY n.updated_at DESC",
    )
    .bind(tag_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Notes whose notebook was deleted (notebook_id is NULL) but not trashed.
pub(crate) async fn list_unfiled(pool: &SqlitePool) -> Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE notebook_id IS NULL AND is_trashed = 0 ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Trashed notes, newest first.
pub(crate) async fn list_trashed(pool: &SqlitePool) -> Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE is_trashed = 1 ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Turn a zero `rows_affected` into the caller-supplied typed NotFound.
pub(crate) fn ensure_affected(
    rows_affected: u64,
    not_found: impl FnOnce() -> crate::core::error::Error,
) -> Result<()> {
    if rows_affected == 0 {
        Err(not_found())
    } else {
        Ok(())
    }
}
