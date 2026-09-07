//! Notebook rows: tree operations and path resolution.

use sqlx::SqlitePool;

use crate::core::error::{Error, Result};
use crate::core::model::{Notebook, NotebookId};
use crate::core::repository::notes::ensure_affected;

/// Validate a notebook display name and return its trimmed form.
///
/// The name becomes a path segment in several places with different rules:
/// a `'/'` would read as a nesting separator during sync, and a `'\\'`
/// would collide with file separators on export (and is already rejected
/// in sync uuids). Rejecting both up front keeps the three
/// representations from silently diverging; empty names are rejected too.
/// The trimmed value is stored so names never carry accidental padding.
fn validate_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidInput(
            "notebook name must not be empty".into(),
        ));
    }
    if trimmed.contains(['/', '\\']) {
        return Err(Error::InvalidInput(
            "notebook name must not contain '/' or '\\'".into(),
        ));
    }
    Ok(trimmed.to_owned())
}

/// Turn a UNIQUE-constraint failure (a sibling notebook already carries
/// this name) into the typed duplicate-name error; any other failure
/// passes through unchanged.
fn map_duplicate(err: sqlx::Error, name: &str) -> Error {
    match err {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            Error::NotebookNameExists(name.to_owned())
        }
        other => other.into(),
    }
}

/// Insert a notebook and return the created row.
pub(crate) async fn create(
    pool: &SqlitePool,
    parent_id: Option<NotebookId>,
    name: &str,
) -> Result<Notebook> {
    let name = validate_name(name)?;
    let row = sqlx::query_as::<_, Notebook>(
        "INSERT INTO notebooks (parent_id, name) VALUES (?, ?) RETURNING \
         id, parent_id, name, created_at, updated_at",
    )
    .bind(parent_id)
    .bind(&name)
    .fetch_one(pool)
    .await
    .map_err(|err| map_duplicate(err, &name))?;
    Ok(row)
}

/// Rename a notebook and bump its `updated_at`.
pub(crate) async fn rename(pool: &SqlitePool, id: NotebookId, name: &str) -> Result<()> {
    let name = validate_name(name)?;
    let result =
        sqlx::query("UPDATE notebooks SET name = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(&name)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|err| map_duplicate(err, &name))?;
    ensure_affected(result.rows_affected(), || Error::NotebookNotFound(id))
}

/// Delete a notebook. Nested notebooks cascade; its notes become unfiled
/// (`notebook_id` is set to NULL by the schema).
pub(crate) async fn delete(pool: &SqlitePool, id: NotebookId) -> Result<()> {
    let result = sqlx::query("DELETE FROM notebooks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    ensure_affected(result.rows_affected(), || Error::NotebookNotFound(id))
}

/// All notebooks, sorted by name.
pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<Notebook>> {
    let rows = sqlx::query_as::<_, Notebook>(
        "SELECT id, parent_id, name, created_at, updated_at FROM notebooks ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
