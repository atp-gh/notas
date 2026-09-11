//! Resource rows + blob files: Joplin-style attachment storage.
//!
//! Metadata lives in the `resources` table; bytes live in
//! `<data_dir>/resources/<uuid>`. References are parsed from note bodies
//! (`:/<uuid>`), so orphan detection is a content scan — trashed notes count
//! as referencing (restoring must not lose bytes).

use std::collections::HashSet;
use std::path::Path;

use sqlx::SqlitePool;

use crate::core::error::Result;
use crate::core::model::Resource;
use crate::core::resources::{
    MAX_ATTACHMENT_BYTES, guess_mime, is_valid_resource_id, new_resource_id, resource_path,
};

/// Fetch one resource by uuid.
pub(crate) async fn get(pool: &SqlitePool, uuid: &str) -> Result<Option<Resource>> {
    let row = sqlx::query_as::<_, Resource>(
        "SELECT uuid, filename, mime, size, created_at, updated_at \
         FROM resources WHERE uuid = ?",
    )
    .bind(uuid)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// All resources, newest first.
pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<Resource>> {
    let rows = sqlx::query_as::<_, Resource>(
        "SELECT uuid, filename, mime, size, created_at, updated_at \
         FROM resources ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Insert a new attachment from a source file: validates the 100 MiB cap,
/// copies bytes into `dir/<uuid>`, and records the metadata row. A caller
/// supplied `uuid` (Joplin re-import with an id prefix) is honored when
/// valid; otherwise a fresh id is generated.
pub(crate) async fn add_file(
    pool: &SqlitePool,
    dir: &Path,
    source: &Path,
    filename: &str,
    uuid: Option<&str>,
) -> Result<Resource> {
    let meta = tokio::fs::metadata(source).await?;
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err(crate::core::error::Error::InvalidInput(format!(
            "attachment {} is {} bytes, exceeding the 100 MiB limit",
            filename,
            meta.len()
        )));
    }
    let bytes = tokio::fs::read(source).await?;
    add_bytes(pool, dir, &bytes, filename, uuid).await
}

/// Insert a new attachment from in-memory bytes (clipboard paste, tests).
pub(crate) async fn add_bytes(
    pool: &SqlitePool,
    dir: &Path,
    bytes: &[u8],
    filename: &str,
    uuid: Option<&str>,
) -> Result<Resource> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ATTACHMENT_BYTES {
        return Err(crate::core::error::Error::InvalidInput(format!(
            "attachment {filename} exceeds the 100 MiB limit"
        )));
    }
    let id = match uuid {
        Some(id) if is_valid_resource_id(id) => id.to_string(),
        _ => new_resource_id(),
    };
    tokio::fs::create_dir_all(dir).await?;
    let dest = resource_path(dir, &id);
    tokio::fs::write(&dest, bytes).await?;
    let mime = guess_mime(filename);
    let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let row = sqlx::query_as::<_, Resource>(
        "INSERT INTO resources (uuid, filename, mime, size, updated_at) \
         VALUES (?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(uuid) DO UPDATE SET filename = excluded.filename, \
         mime = excluded.mime, size = excluded.size, updated_at = datetime('now') \
         RETURNING uuid, filename, mime, size, created_at, updated_at",
    )
    .bind(&id)
    .bind(filename)
    .bind(&mime)
    .bind(size)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Upsert metadata for a resource whose bytes were already written (import
/// and sync paths that stream the blob themselves).
pub(crate) async fn upsert_meta(
    pool: &SqlitePool,
    uuid: &str,
    filename: &str,
    mime: &str,
    size: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO resources (uuid, filename, mime, size, updated_at) \
         VALUES (?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(uuid) DO UPDATE SET filename = excluded.filename, \
         mime = excluded.mime, size = excluded.size, updated_at = datetime('now')",
    )
    .bind(uuid)
    .bind(filename)
    .bind(mime)
    .bind(size)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a resource row + blob file. Idempotent: missing rows/files are
/// not errors (GC races and re-entrant syncs rely on this).
pub(crate) async fn remove(pool: &SqlitePool, dir: &Path, uuid: &str) -> Result<()> {
    sqlx::query("DELETE FROM resources WHERE uuid = ?")
        .bind(uuid)
        .execute(pool)
        .await?;
    let path = resource_path(dir, uuid);
    match tokio::fs::remove_file(&path).await {
        Ok(()) | Err(_) => Ok(()),
    }
}

/// Every `:/<id>` referenced by any note (trashed included — restoring a
/// note must not find its bytes gone).
pub(crate) async fn referenced_ids(pool: &SqlitePool) -> Result<HashSet<String>> {
    let contents: Vec<String> = sqlx::query_scalar("SELECT content FROM notes")
        .fetch_all(pool)
        .await?;
    Ok(crate::core::resources::referenced_ids(
        contents.iter().map(String::as_str),
    ))
}
