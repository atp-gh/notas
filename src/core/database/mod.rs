//! SQLite lifecycle: connection setup, schema creation, migrations.

mod migrations;
mod schema;

pub use schema::SCHEMA_SQL;

use std::path::Path;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::core::error::Result;

/// Open (creating if needed) the database, bring the schema to the current
/// version, and rebuild the FTS index if it is out of sync with the notes
/// table.
///
/// # Errors
///
/// Returns [`crate::core::error::Error::Io`] when the parent directory
/// cannot be created, [`crate::core::error::Error::Migration`] when the
/// stored schema cannot be brought to the current version, and
/// [`crate::core::error::Error::Database`] for other SQL failures.
pub async fn connect(db_path: impl AsRef<Path>) -> Result<SqlitePool> {
    let path = db_path.as_ref().to_path_buf();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5))
        .journal_mode(SqliteJournalMode::Wal);

    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;

    initialize(&pool).await?;

    Ok(pool)
}

/// Create missing schema objects, apply ordered migrations, and verify the
/// FTS index. Idempotent: repeated start-ups change nothing.
async fn initialize(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA_SQL).execute(pool).await?;

    // Seed the version table for fresh databases (the DDL insert above is
    // conditional; migrations.rs then only applies steps newer than the
    // recorded version).
    let mut conn = pool.acquire().await?;
    migrations::run(&mut conn).await?;
    drop(conn);

    // A trigger gap (schema older than the FTS table, or a crash between
    // the two) desynchronizes the external-content index; rebuild makes it
    // consistent again. `INSERT INTO notes_fts(notes_fts) VALUES('rebuild')`
    // is a no-op when the index already matches.
    rebuild_fts_if_needed(pool).await?;

    Ok(())
}

/// Rebuild the FTS index when its row count disagrees with `notes`. The
/// external-content table's internal count is exposed by a direct scan of
/// `notes_fts`, which is exact after a rebuild.
async fn rebuild_fts_if_needed(pool: &SqlitePool) -> Result<()> {
    let note_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes")
        .fetch_one(pool)
        .await?;
    let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes_fts")
        .fetch_one(pool)
        .await?;
    if note_count != fts_count {
        sqlx::query("INSERT INTO notes_fts(notes_fts) VALUES ('rebuild')")
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_twice_is_idempotent_and_versions_the_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.db");
        let pool = connect(&path).await.unwrap();
        pool.close().await;

        // Re-open: schema DDL and migrations must not fail or duplicate.
        let pool = connect(&path).await.unwrap();
        let version: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, 2, "fresh database starts at the current version");
        pool.close().await;
    }

    #[tokio::test]
    async fn repeated_connect_does_not_duplicate_schema_objects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.db");
        let pool = connect(&path).await.unwrap();
        pool.close().await;

        let pool = connect(&path).await.unwrap();
        let notebooks: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'notebooks'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(notebooks, 1);
        pool.close().await;
    }

    #[tokio::test]
    async fn fts_index_is_rebuilt_when_desynchronized() {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(dir.path().join("a.db")).await.unwrap();

        sqlx::query("INSERT INTO notes (title, content) VALUES ('hello', 'world')")
            .execute(&pool)
            .await
            .unwrap();
        // Simulate a desynchronized index by deleting the FTS row behind
        // the trigger's back.
        sqlx::query("DELETE FROM notes_fts")
            .execute(&pool)
            .await
            .unwrap();

        // A fresh connect() notices the mismatch and rebuilds.
        pool.close().await;
        let pool = connect(dir.path().join("a.db")).await.unwrap();
        let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes_fts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fts_count, 1, "FTS must be rebuilt to match notes");
        pool.close().await;
    }
}
