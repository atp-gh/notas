//! Repository layer: every database operation in one place.
//!
//! All functions are async and take a `&SqlitePool`; they never block the
//! caller's thread. The GTK layer talks to this module only through its
//! async worker, never directly on the UI thread.

use std::path::Path;

use sqlx::{AssertSqlSafe, FromRow, SqlitePool};

use crate::core::Result;
use crate::core::notes::models::{Note, Notebook, SearchHit, Tag, TagCount};
use crate::core::sync::{LocalNote, Sidecar};

// ---------------------------------------------------------------------------
// Notebooks
// ---------------------------------------------------------------------------

/// Insert a notebook and return the created row.
pub async fn create_notebook(
    pool: &SqlitePool,
    parent_id: Option<i64>,
    name: &str,
) -> Result<Notebook> {
    let row = sqlx::query(
        "INSERT INTO notebooks (parent_id, name) VALUES (?, ?) RETURNING \
         id, parent_id, name, created_at, updated_at",
    )
    .bind(parent_id)
    .bind(name)
    .fetch_one(pool)
    .await?;
    Ok(Notebook::from_row(&row)?)
}

/// Rename a notebook and bump its `updated_at`.
pub async fn rename_notebook(pool: &SqlitePool, id: i64, name: &str) -> Result<()> {
    sqlx::query("UPDATE notebooks SET name = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete a notebook. Nested notebooks cascade; its notes become unfiled
/// (`notebook_id` is set to NULL by the schema).
pub async fn delete_notebook(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM notebooks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// All notebooks, sorted by name.
pub async fn list_notebooks(pool: &SqlitePool) -> Result<Vec<Notebook>> {
    let rows = sqlx::query_as::<_, Notebook>(
        "SELECT id, parent_id, name, created_at, updated_at FROM notebooks ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------------

/// Insert a note (with the given title) and return the created row.
pub async fn create_note(pool: &SqlitePool, notebook_id: Option<i64>, title: &str) -> Result<Note> {
    let row = sqlx::query(
        "INSERT INTO notes (notebook_id, title) VALUES (?, ?) RETURNING \
         id, notebook_id, title, content, is_trashed, created_at, updated_at",
    )
    .bind(notebook_id)
    .bind(title)
    .fetch_one(pool)
    .await?;
    Ok(Note::from_row(&row)?)
}

/// Fetch a single note by id, if it exists.
pub async fn get_note(pool: &SqlitePool, id: i64) -> Result<Option<Note>> {
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
pub async fn update_note(pool: &SqlitePool, id: i64, title: &str, content: &str) -> Result<()> {
    sqlx::query(
        "UPDATE notes SET title = ?, content = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(title)
    .bind(content)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Move a note to the trash.
pub async fn trash_note(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE notes SET is_trashed = 1, updated_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Restore a trashed note.
pub async fn restore_note(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE notes SET is_trashed = 0, updated_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Physically delete a note (FTS row removed by trigger `notes_ad`).
///
/// If the note had a sync uuid, a tombstone is recorded so the deletion
/// propagates to other devices (they trash their copy) instead of the
/// note resurrecting from the remote store on the next sync.
pub async fn delete_note_forever(pool: &SqlitePool, id: i64) -> Result<()> {
    let mut tx = pool.begin().await?;
    let uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM notes WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM notes WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if let Some(uuid) = uuid {
        sqlx::query("INSERT OR IGNORE INTO sync_tombstones (uuid) VALUES (?)")
            .bind(uuid)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Non-trashed notes of one notebook, newest first.
pub async fn list_notes(pool: &SqlitePool, notebook_id: i64) -> Result<Vec<Note>> {
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
pub async fn list_all_notes(pool: &SqlitePool) -> Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE is_trashed = 0 ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// All non-trashed notes carrying a given tag, newest first.
pub async fn list_notes_by_tag(pool: &SqlitePool, tag_id: i64) -> Result<Vec<Note>> {
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
pub async fn list_unfiled_notes(pool: &SqlitePool) -> Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE notebook_id IS NULL AND is_trashed = 0 ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Trashed notes, newest first.
pub async fn list_trashed(pool: &SqlitePool) -> Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE is_trashed = 1 ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Search (FTS5)
// ---------------------------------------------------------------------------

/// Escape free-form user input into a safe FTS5 MATCH string: each
/// whitespace-separated token becomes a quoted phrase.
/// Full-text search over title + content, newest first, capped at 200 hits.
pub async fn search(pool: &SqlitePool, query: &str) -> Result<Vec<SearchHit>> {
    let match_expr = crate::core::search::fts_query(query);
    if match_expr.trim().is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, SearchHit>(
        "SELECT n.id, n.title, \
                snippet(notes_fts, 1, '⟪', '⟫', '…', 14) AS snippet \
         FROM notes_fts \
         JOIN notes n ON n.id = notes_fts.rowid \
         WHERE notes_fts MATCH ? AND n.is_trashed = 0 \
         ORDER BY rank LIMIT 200",
    )
    .bind(match_expr)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// All tags with the number of notes using each, sorted by name.
pub async fn list_tags(pool: &SqlitePool) -> Result<Vec<TagCount>> {
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
pub async fn get_note_tags(pool: &SqlitePool, note_id: i64) -> Result<Vec<Tag>> {
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
pub async fn set_note_tags(pool: &SqlitePool, note_id: i64, names: &[String]) -> Result<()> {
    let mut tx = pool.begin().await?;

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
pub async fn rename_tag(pool: &SqlitePool, id: i64, name: &str) -> Result<()> {
    sqlx::query("UPDATE tags SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete a tag; its `note_tags` links are removed by CASCADE.
pub async fn delete_tag(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM tags WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Export & backup
// ---------------------------------------------------------------------------

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('-');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Export every non-trashed note as `.md` files, grouped into notebook
/// subdirectories. The file name is the note title; collisions get a
/// numeric suffix. Returns the number of notes written.
pub async fn export_markdown(pool: &SqlitePool, out_dir: &Path) -> Result<usize> {
    use std::collections::HashMap;

    tokio::fs::create_dir_all(out_dir).await?;

    let notebooks = list_notebooks(pool).await?;
    let mut dir_by_id: HashMap<i64, String> = HashMap::new();
    for nb in &notebooks {
        let dir = out_dir.join(sanitize_filename(&nb.name));
        tokio::fs::create_dir_all(&dir).await?;
        dir_by_id.insert(nb.id, dir.to_string_lossy().into_owned());
    }

    let notes = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE is_trashed = 0 ORDER BY notebook_id, updated_at",
    )
    .fetch_all(pool)
    .await?;

    let mut written = 0usize;
    let mut used: HashMap<String, usize> = HashMap::new();
    for note in &notes {
        let dir = note
            .notebook_id
            .and_then(|id| dir_by_id.get(&id).cloned())
            .unwrap_or_else(|| out_dir.to_string_lossy().into_owned());

        let base = sanitize_filename(&note.title);
        let count = used.entry(base.clone()).or_insert(0);
        let name = if *count == 0 {
            format!("{base}.md")
        } else {
            format!("{base}-{}.md", *count)
        };
        *count += 1;

        let path = std::path::Path::new(&dir).join(name);
        let frontmatter = format!(
            "---\ntitle: {}\ncreated: {}\nupdated: {}\n---\n\n",
            note.title.replace('\n', " "),
            note.created_at,
            note.updated_at
        );
        tokio::fs::write(&path, format!("{frontmatter}{}", note.content)).await?;
        written += 1;
    }

    Ok(written)
}

/// Create a consistent snapshot of the database at `dest` using
/// `VACUUM INTO` (SQLite >= 3.27).
pub async fn backup(pool: &SqlitePool, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // `VACUUM INTO` takes a filename literal and cannot use a bind
    // parameter, so the path is embedded directly. The path comes from
    // the user's own file-save dialog, and every single quote is escaped
    // (SQLite string-literal escaping), so it cannot break out of the
    // string literal; audited safe, hence `AssertSqlSafe`.
    let escaped = dest.to_string_lossy().replace('\'', "''");
    let sql = format!("VACUUM INTO '{}'", escaped);
    sqlx::query(AssertSqlSafe(sql)).execute(pool).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

/// Assign a uuid to every note that does not have one yet (notes created
/// before the sync feature, or before the first sync). Returns the number
/// of uuids assigned. Idempotent.
pub async fn ensure_note_uuids(pool: &SqlitePool) -> Result<usize> {
    let result =
        sqlx::query("UPDATE notes SET uuid = lower(hex(randomblob(16))) WHERE uuid IS NULL")
            .execute(pool)
            .await?;
    Ok(result.rows_affected() as usize)
}

/// Build the full local index the planner needs: every note (trashed
/// included) with an assigned uuid, notebook path and tag names.
pub async fn sync_local_index(pool: &SqlitePool) -> Result<Vec<LocalNote>> {
    use std::collections::HashMap;

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
        if segments.is_empty() {
            None
        } else {
            Some(segments.join("/"))
        }
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

/// Apply a remote note locally: create it if the uuid is unknown, update
/// it otherwise. The remote `updated_at` is preserved verbatim so the next
/// sync sees an identical pair instead of re-uploading.
pub async fn apply_remote_note(pool: &SqlitePool, sidecar: &Sidecar, content: &str) -> Result<()> {
    let notebook_id = find_or_create_notebook_path(pool, sidecar.notebook.as_deref()).await?;
    let note_id: i64 = match sqlx::query_scalar::<_, i64>("SELECT id FROM notes WHERE uuid = ?")
        .bind(&sidecar.uuid)
        .fetch_optional(pool)
        .await?
    {
        Some(id) => {
            sqlx::query(
                "UPDATE notes SET notebook_id = ?, title = ?, content = ?, \
                 is_trashed = ?, updated_at = ? WHERE uuid = ?",
            )
            .bind(notebook_id)
            .bind(&sidecar.title)
            .bind(content)
            .bind(sidecar.trashed)
            .bind(&sidecar.updated_at)
            .bind(&sidecar.uuid)
            .execute(pool)
            .await?;
            id
        }
        None => {
            sqlx::query_scalar::<_, i64>(
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
            .fetch_one(pool)
            .await?
        }
    };
    set_note_tags(pool, note_id, &sidecar.tags).await?;
    Ok(())
}

/// Create a local copy of the remote version of a note that collided with
/// the local version at the same timestamp. The copy gets a fresh uuid
/// (it will upload as a new note next sync), a `(conflict copy)` title
/// suffix, and the remote body — so both texts survive on both devices.
pub async fn create_conflict_copy(
    pool: &SqlitePool,
    sidecar: &Sidecar,
    content: &str,
) -> Result<()> {
    let notebook_id = find_or_create_notebook_path(pool, sidecar.notebook.as_deref()).await?;
    let title = format!("{} (conflict copy)", sidecar.title);
    let note_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO notes (uuid, notebook_id, title, content, is_trashed, \
         created_at, updated_at) \
         VALUES (lower(hex(randomblob(16))), ?, ?, ?, 0, ?, ?) RETURNING id",
    )
    .bind(notebook_id)
    .bind(&title)
    .bind(content)
    .bind(&sidecar.updated_at)
    .bind(&sidecar.updated_at)
    .fetch_one(pool)
    .await?;
    set_note_tags(pool, note_id, &sidecar.tags).await?;
    Ok(())
}

/// Move a note to the trash without bumping its `updated_at`, so a
/// tombstone reaction doesn't make the local copy look newer than the
/// tombstone on the next sync.
pub async fn trash_note_by_uuid_no_bump(pool: &SqlitePool, uuid: &str) -> Result<()> {
    sqlx::query("UPDATE notes SET is_trashed = 1 WHERE uuid = ?")
        .bind(uuid)
        .execute(pool)
        .await?;
    Ok(())
}

/// Every locally recorded tombstone: `(uuid, deleted_at)` pairs for notes
/// permanently deleted on this device.
pub async fn list_tombstones(pool: &SqlitePool) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT uuid, deleted_at FROM sync_tombstones ORDER BY deleted_at",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Resolve a notebook path (`"Parent/Child"`) to its leaf notebook id,
/// creating the whole chain on demand. `None` resolves to `None` (unfiled).
async fn find_or_create_notebook_path(
    pool: &SqlitePool,
    path: Option<&str>,
) -> Result<Option<i64>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let mut parent_id: Option<i64> = None;
    for segment in path.split('/') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM notebooks WHERE parent_id IS ? AND name = ?",
        )
        .bind(parent_id)
        .bind(segment)
        .fetch_optional(pool)
        .await?;
        parent_id = Some(match id {
            Some(id) => id,
            None => {
                sqlx::query_scalar::<_, i64>(
                    "INSERT INTO notebooks (parent_id, name) VALUES (?, ?) RETURNING id",
                )
                .bind(parent_id)
                .bind(segment)
                .fetch_one(pool)
                .await?
            }
        });
    }
    Ok(parent_id)
}
