//! Database connection setup: path resolution, pool creation, schema init.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::Result;
use crate::schema::SCHEMA_SQL;

/// Base data directory: `$XDG_DATA_HOME/notas`, falling back to
/// `~/.local/share/notas` when the variable is unset or empty.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME")
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("notas");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/share/notas")
}

/// Location of the main SQLite database file.
pub fn db_path() -> PathBuf {
    data_dir().join("notas.db")
}

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

    Ok(pool)
}
