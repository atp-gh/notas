//! Database connection setup: path resolution, pool creation, schema init.

use std::path::{Path, PathBuf};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteJournalMode};
use sqlx::SqlitePool;

use crate::schema::SCHEMA_SQL;

pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("notas");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/share/notas")
}

pub fn db_path() -> PathBuf {
    data_dir().join("notas.db")
}

/// Open (creating if needed) the database and apply the schema.
pub async fn connect(db_path: impl AsRef<Path>) -> anyhow::Result<SqlitePool> {
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

    Ok(pool)
}
