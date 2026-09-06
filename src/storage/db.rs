//! Database connection setup: path resolution, pool creation, schema init.

use std::path::Path;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::core::Result;
use crate::storage::schema::SCHEMA_SQL;

/// Open (creating if needed) the database and apply the schema.
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

    sqlx::query(SCHEMA_SQL).execute(&pool).await?;

    // v0.2 migration: the `notes.uuid` sync-identity column. Fresh
    // databases get it from `SCHEMA_SQL`; pre-existing ones need an
    // `ALTER TABLE`. The unique index lives here (not in `SCHEMA_SQL`)
    // because it must only be created once the column actually exists.
    migrate_uuid_column(&pool).await?;

    Ok(pool)
}

/// Ensure the `notes.uuid` column exists, adding it to databases created
/// before the sync feature. Idempotent and safe to run on every start.
async fn migrate_uuid_column(pool: &SqlitePool) -> Result<()> {
    let has_uuid = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'uuid'",
    )
    .fetch_one(pool)
    .await?;
    if has_uuid == 0 {
        sqlx::query("ALTER TABLE notes ADD COLUMN uuid TEXT")
            .execute(pool)
            .await?;
    }
    // NULLs are allowed (and distinct) in SQLite unique indexes, so notes
    // that have not been synced yet don't collide.
    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_uuid ON notes(uuid)")
        .execute(pool)
        .await?;
    Ok(())
}
