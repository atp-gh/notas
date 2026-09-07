//! Notebook rows: tree operations and path resolution.

use sqlx::SqlitePool;

use crate::core::Result;
use crate::core::model::{Notebook, NotebookId};
use crate::core::repository::notes::ensure_affected;

/// Insert a notebook and return the created row.
pub(crate) async fn create(
    pool: &SqlitePool,
    parent_id: Option<NotebookId>,
    name: &str,
) -> Result<Notebook> {
    let row = sqlx::query_as::<_, Notebook>(
        "INSERT INTO notebooks (parent_id, name) VALUES (?, ?) RETURNING \
         id, parent_id, name, created_at, updated_at",
    )
    .bind(parent_id)
    .bind(name)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Rename a notebook and bump its `updated_at`.
pub(crate) async fn rename(pool: &SqlitePool, id: NotebookId, name: &str) -> Result<()> {
    let result =
        sqlx::query("UPDATE notebooks SET name = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(name)
            .bind(id)
            .execute(pool)
            .await?;
    ensure_affected(result.rows_affected(), || {
        crate::core::Error::NotebookNotFound(id)
    })
}

/// Delete a notebook. Nested notebooks cascade; its notes become unfiled
/// (`notebook_id` is set to NULL by the schema).
pub(crate) async fn delete(pool: &SqlitePool, id: NotebookId) -> Result<()> {
    let result = sqlx::query("DELETE FROM notebooks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    ensure_affected(result.rows_affected(), || {
        crate::core::Error::NotebookNotFound(id)
    })
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
