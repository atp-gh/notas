//! Repository layer: every database operation in one place.
//!
//! All functions are async and take a `&SqlitePool`; they never block the
//! caller's thread. The GTK layer talks to this module only through its
//! async worker, never directly on the UI thread.

use std::path::Path;

use sqlx::{FromRow, SqlitePool};

use crate::models::{Notebook, Note, SearchHit, Tag, TagCount};

// ---------------------------------------------------------------------------
// Notebooks
// ---------------------------------------------------------------------------

pub async fn create_notebook(
    pool: &SqlitePool,
    parent_id: Option<i64>,
    name: &str,
) -> anyhow::Result<Notebook> {
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

pub async fn rename_notebook(pool: &SqlitePool, id: i64, name: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE notebooks SET name = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_notebook(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM notebooks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_notebooks(pool: &SqlitePool) -> anyhow::Result<Vec<Notebook>> {
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

pub async fn create_note(
    pool: &SqlitePool,
    notebook_id: Option<i64>,
    title: &str,
) -> anyhow::Result<Note> {
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

pub async fn get_note(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<Note>> {
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
pub async fn update_note(
    pool: &SqlitePool,
    id: i64,
    title: &str,
    content: &str,
) -> anyhow::Result<()> {
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

pub async fn trash_note(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE notes SET is_trashed = 1, updated_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn restore_note(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE notes SET is_trashed = 0, updated_at = datetime('now') WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Physically delete a note (FTS row removed by trigger `notes_ad`).
pub async fn delete_note_forever(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM notes WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_notes(pool: &SqlitePool, notebook_id: i64) -> anyhow::Result<Vec<Note>> {
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
pub async fn list_all_notes(pool: &SqlitePool) -> anyhow::Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE is_trashed = 0 ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// All non-trashed notes carrying a given tag, newest first.
pub async fn list_notes_by_tag(pool: &SqlitePool, tag_id: i64) -> anyhow::Result<Vec<Note>> {
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
pub async fn list_unfiled_notes(pool: &SqlitePool) -> anyhow::Result<Vec<Note>> {
    let rows = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE notebook_id IS NULL AND is_trashed = 0 ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_trashed(pool: &SqlitePool) -> anyhow::Result<Vec<Note>> {
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
fn fts_query(user_input: &str) -> String {
    user_input
        .split_whitespace()
        .map(|tok| format!("\"{}\"", tok.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn search(pool: &SqlitePool, query: &str) -> anyhow::Result<Vec<SearchHit>> {
    let match_expr = fts_query(query);
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

pub async fn list_tags(pool: &SqlitePool) -> anyhow::Result<Vec<TagCount>> {
    let rows = sqlx::query_as::<_, TagCount>(
        "SELECT t.id, t.name, COUNT(nt.note_id) AS note_count \
         FROM tags t LEFT JOIN note_tags nt ON nt.tag_id = t.id \
         GROUP BY t.id ORDER BY t.name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_note_tags(pool: &SqlitePool, note_id: i64) -> anyhow::Result<Vec<Tag>> {
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
pub async fn set_note_tags(pool: &SqlitePool, note_id: i64, names: &[String]) -> anyhow::Result<()> {
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
        let tag_id: i64 =
            sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
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

pub async fn delete_tag(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
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
        .map(|c| if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '-' })
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
/// numeric suffix.
pub async fn export_markdown(pool: &SqlitePool, out_dir: &Path) -> anyhow::Result<usize> {
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
pub async fn backup(pool: &SqlitePool, dest: &Path) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let escaped = dest.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{}'", escaped))
        .execute(pool)
        .await?;
    Ok(())
}
