//! Tag rows: CRUD, normalization, and note-tag membership.

use sqlx::SqlitePool;

use crate::core::error::Result;
use crate::core::model::{NoteId, Tag, TagCount, TagId};
use crate::core::repository::notes::ensure_affected;

/// All tags with the number of notes using each, sorted by name.
pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<TagCount>> {
    let rows = sqlx::query_as::<_, TagCount>(
        "SELECT t.id, t.name, COUNT(nt.note_id) AS note_count \
         FROM tags t LEFT JOIN note_tags nt ON nt.tag_id = t.id \
         GROUP BY t.id ORDER BY t.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// The tags attached to one note, sorted by name.
pub(crate) async fn get_for_note(pool: &SqlitePool, note_id: NoteId) -> Result<Vec<Tag>> {
    let rows = sqlx::query_as::<_, Tag>(
        "SELECT t.id, t.name FROM tags t \
         JOIN note_tags nt ON nt.tag_id = t.id \
         WHERE nt.note_id = ? ORDER BY t.name",
    )
    .bind(note_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Replace the tag set of a note. Tags are created on demand.
pub(crate) async fn set_for_note(
    pool: &SqlitePool,
    note_id: NoteId,
    names: &[String],
) -> Result<()> {
    let mut tx = pool.begin().await?;

    // The note must exist before we touch its tags, otherwise the delete
    // below silently succeeds against nothing.
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM notes WHERE id = ?")
        .bind(note_id)
        .fetch_optional(&mut *tx)
        .await?;
    exists.ok_or(crate::core::error::Error::NoteNotFound(note_id))?;

    sqlx::query("DELETE FROM note_tags WHERE note_id = ?")
        .bind(note_id)
        .execute(&mut *tx)
        .await?;

    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
            .bind(name)
            .execute(&mut *tx)
            .await?;
        let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
            .bind(name)
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("INSERT OR IGNORE INTO note_tags (note_id, tag_id) VALUES (?, ?)")
            .bind(note_id)
            .bind(tag_id)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Rename a tag. The `tags.name` UNIQUE constraint rejects colliding names.
pub(crate) async fn rename(pool: &SqlitePool, id: TagId, name: &str) -> Result<()> {
    let result = sqlx::query("UPDATE tags SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
    ensure_affected(result.rows_affected(), || {
        crate::core::error::Error::TagNotFound(id)
    })
}

/// Delete a tag; its `note_tags` links are removed by CASCADE.
pub(crate) async fn delete(pool: &SqlitePool, id: TagId) -> Result<()> {
    let result = sqlx::query("DELETE FROM tags WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    ensure_affected(result.rows_affected(), || {
        crate::core::error::Error::TagNotFound(id)
    })
}
