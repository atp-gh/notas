//! SQLite snapshot boundary.
//!
//! `VACUUM INTO` takes a filename *literal* and cannot use a bind
//! parameter, so the path is embedded directly. The path comes from the
//! user's own file-save dialog, and every single quote is escaped
//! (SQLite string-literal escaping), so it cannot break out of the string
//! literal; audited safe, hence `AssertSqlSafe`.

use std::path::Path;

use sqlx::{AssertSqlSafe, SqlitePool};

use crate::core::Result;

/// Create a consistent snapshot of the database at `dest` using
/// `VACUUM INTO` (SQLite >= 3.27).
pub(crate) async fn run(pool: &SqlitePool, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let escaped = dest.to_string_lossy().replace('\'', "''");
    let sql = format!("VACUUM INTO '{}'", escaped);
    sqlx::query(AssertSqlSafe(sql)).execute(pool).await?;
    Ok(())
}
