//! Sync state: uuid assignment, local sync index, tombstones, and the
//! remote-note application path.

use std::collections::HashMap;

use sqlx::SqlitePool;

use crate::core::Result;
use crate::core::model::NoteId;
use crate::core::sync::{LocalNote, Sidecar};

/// Assign a uuid to every note that does not have one yet (notes created
/// before the sync feature, or before the first sync). Returns the number
/// of uuids assigned. Idempotent.
pub(crate) async fn ensure_note_uuids(pool: &SqlitePool) -> Result<usize> {
    let result =
        sqlx::query("UPDATE notes SET uuid = lower(hex(randomblob(16))) WHERE uuid IS NULL")
            .execute(pool)
            .await?;
    Ok(result.rows_affected() as usize)
}

/// Build the full local index the planner needs: every note (trashed
/// included) with an assigned uuid, notebook path and tag names.
pub(crate) async fn local_index(pool: &SqlitePool) -> Result<Vec<LocalNote>> {
    // Notebook hierarchy: id -> (name, parent_id).
    let notebooks = sqlx::query_as::<_, (i64, Option<i64>, String)>(
        "SELECT id, parent_id, name FROM notebooks",
    )
    .fetch_all(pool)
    .await?;
    let name_by_id: HashMap<i64, String> = notebooks
        .iter()
        .map(|(id, _, name)| (*id, name.clone()))
        .collect();
    let parent_by_id: HashMap<i64, Option<i64>> = notebooks
        .iter()
        .map(|(id, parent, _)| (*id, *parent))
        .collect();

    // Notebook path "Parent/Child" by walking the parent chain.
    let path_for = |id: Option<i64>| -> Option<String> {
        let mut segments = Vec::new();
        let mut current = id;
        let mut guard = 0;
        while let Some(nb_id) = current {
            let Some(name) = name_by_id.get(&nb_id) else {
                break;
            };
            segments.push(name.clone());
            current = parent_by_id.get(&nb_id).copied().flatten();
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        segments.reverse();
        (!segments.is_empty()).then(|| segments.join("/"))
    };

    let rows = sqlx::query_as::<
        _,
        (
            i64,
            Option<i64>,
            String,
            String,
            bool,
            String,
            Option<String>,
        ),
    >(
        "SELECT id, notebook_id, title, content, is_trashed, updated_at, uuid \
         FROM notes WHERE uuid IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;

    // Tags per note, in one query.
    let tag_rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT nt.note_id, t.name FROM note_tags nt \
         JOIN tags t ON t.id = nt.tag_id ORDER BY t.name",
    )
    .fetch_all(pool)
    .await?;
    let mut tags_by_note: HashMap<i64, Vec<String>> = HashMap::new();
    for (note_id, name) in tag_rows {
        tags_by_note.entry(note_id).or_default().push(name);
    }

    Ok(rows
        .into_iter()
        .map(
            |(id, notebook_id, title, content, is_trashed, updated_at, uuid)| LocalNote {
                uuid: uuid.unwrap_or_default(),
                title,
                content,
                is_trashed,
                updated_at,
                notebook: path_for(notebook_id),
                tags: tags_by_note.remove(&id).unwrap_or_default(),
            },
        )
        .collect())
}

/// Every locally recorded tombstone: `(uuid, deleted_at)` pairs for notes
/// permanently deleted on this device.
pub(crate) async fn list_tombstones(pool: &SqlitePool) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT uuid, deleted_at FROM sync_tombstones ORDER BY deleted_at",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Resolve a notebook path (`"Parent/Child"`) to its leaf notebook id,
/// creating the whole chain on demand. `None` resolves to `None` (unfiled).
pub(crate) async fn find_or_create_notebook_path(
    executor: &mut sqlx::SqliteConnection,
    path: Option<&str>,
) -> Result<Option<crate::core::model::NotebookId>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let mut parent_id: Option<crate::core::model::NotebookId> = None;
    for segment in path.split('/') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let id: Option<crate::core::model::NotebookId> =
            sqlx::query_scalar("SELECT id FROM notebooks WHERE parent_id IS ? AND name = ?")
                .bind(parent_id)
                .bind(segment)
                .fetch_optional(&mut *executor)
                .await?;
        parent_id = Some(match id {
            Some(id) => id,
            None => {
                sqlx::query_scalar::<_, crate::core::model::NotebookId>(
                    "INSERT INTO notebooks (parent_id, name) VALUES (?, ?) RETURNING id",
                )
                .bind(parent_id)
                .bind(segment)
                .fetch_one(&mut *executor)
                .await?
            }
        });
    }
    Ok(parent_id)
}

/// Apply a remote note locally: create it if the uuid is unknown, update
/// it otherwise. The remote `updated_at` is preserved verbatim so the next
/// sync sees an identical pair instead of re-uploading.
///
/// Notebook resolution, the note upsert and the tag replacement run in one
/// transaction: a mid-way failure rolls the whole remote application back
/// instead of leaving a half-applied note behind.
pub(crate) async fn apply_remote_note(
    pool: &SqlitePool,
    sidecar: &Sidecar,
    content: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let notebook_id = find_or_create_notebook_path(&mut tx, sidecar.notebook.as_deref()).await?;
    let note_id: NoteId =
        match sqlx::query_scalar::<_, NoteId>("SELECT id FROM notes WHERE uuid = ?")
            .bind(&sidecar.uuid)
            .fetch_optional(&mut *tx)
            .await?
        {
            Some(id) => {
                let updated = sqlx::query(
                    "UPDATE notes SET notebook_id = ?, title = ?, content = ?, \
                 is_trashed = ?, updated_at = ? WHERE uuid = ?",
                )
                .bind(notebook_id)
                .bind(&sidecar.title)
                .bind(content)
                .bind(sidecar.trashed)
                .bind(&sidecar.updated_at)
                .bind(&sidecar.uuid)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if updated == 0 {
                    // The uuid vanished between the SELECT and the UPDATE
                    // inside the same transaction: treat as a hard failure.
                    return Err(crate::core::Error::InvalidInput(format!(
                        "remote note {} vanished mid-apply",
                        sidecar.uuid
                    )));
                }
                id
            }
            None => {
                sqlx::query_scalar::<_, NoteId>(
                    "INSERT INTO notes (uuid, notebook_id, title, content, is_trashed, \
             created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
                )
                .bind(&sidecar.uuid)
                .bind(notebook_id)
                .bind(&sidecar.title)
                .bind(content)
                .bind(sidecar.trashed)
                .bind(&sidecar.updated_at)
                .bind(&sidecar.updated_at)
                .fetch_one(&mut *tx)
                .await?
            }
        };
    replace_note_tags(&mut tx, note_id, &sidecar.tags).await?;
    tx.commit().await?;
    Ok(())
}

/// Create a local copy of the remote version of a note that collided with
/// the local version at the same timestamp. The copy gets a fresh uuid
/// (it will upload as a new note next sync), a `(conflict copy)` title
/// suffix, and the remote body — so both texts survive on both devices.
pub(crate) async fn create_conflict_copy(
    pool: &SqlitePool,
    sidecar: &Sidecar,
    content: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let notebook_id = find_or_create_notebook_path(&mut tx, sidecar.notebook.as_deref()).await?;
    let title = format!("{} (conflict copy)", sidecar.title);
    let note_id = sqlx::query_scalar::<_, NoteId>(
        "INSERT INTO notes (uuid, notebook_id, title, content, is_trashed, \
         created_at, updated_at) \
         VALUES (lower(hex(randomblob(16))), ?, ?, ?, 0, ?, ?) RETURNING id",
    )
    .bind(notebook_id)
    .bind(&title)
    .bind(content)
    .bind(&sidecar.updated_at)
    .bind(&sidecar.updated_at)
    .fetch_one(&mut *tx)
    .await?;
    replace_note_tags(&mut tx, note_id, &sidecar.tags).await?;
    tx.commit().await?;
    Ok(())
}

/// Move a note to the trash without bumping its `updated_at`, so a
/// tombstone reaction doesn't make the local copy look newer than the
/// tombstone on the next sync.
pub(crate) async fn trash_note_by_uuid_no_bump(pool: &SqlitePool, uuid: &str) -> Result<()> {
    sqlx::query("UPDATE notes SET is_trashed = 1 WHERE uuid = ?")
        .bind(uuid)
        .execute(pool)
        .await?;
    Ok(())
}

/// Replace a note's tags inside an open transaction. Tags are created on
/// demand; empty names are skipped.
async fn replace_note_tags(
    tx: &mut sqlx::SqliteConnection,
    note_id: NoteId,
    names: &[String],
) -> Result<()> {
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
    Ok(())
}
