//! Ordered, idempotent schema migrations for existing databases.
//!
//! Fresh databases are created directly at the current version by
//! [`SCHEMA_SQL`]; databases created before the version table existed are
//! brought up step by step. Every migration runs inside a transaction and
//! is guarded by its recorded version, so repeated start-ups never apply
//! the same change twice.

use sqlx::{Connection, SqliteConnection};

use crate::core::error::Result;

/// One ordered schema upgrade step.
struct Migration {
    /// Version this step migrates *to* (one more than the previous step).
    version: i64,
    /// Human-readable purpose; not printed (the data core stays silent),
    /// but asserted in test messages and read when maintaining migrations.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "documentation for maintainers, not runtime output"
        )
    )]
    description: &'static str,
    /// The SQL to run; may contain multiple statements.
    sql: &'static str,
}

/// All migrations, ordered by `version`. A database at version `n` runs
/// every step with `version > n`, in list order.
const MIGRATIONS: &[Migration] = &[
    // v0 -> v1: pre-versioning databases only carry the v0.2 hand-written
    // `notes.uuid` tweak; the unique index is the durable marker. Fresh
    // databases created by `SCHEMA_SQL` are already at version 1.
    Migration {
        version: 1,
        description: "add notes.uuid sync identity and its unique index",
        sql: "ALTER TABLE notes ADD COLUMN uuid TEXT;\n\
              CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_uuid ON notes(uuid);",
    },
    // v1 -> v2: notebook trash. The old sibling-uniqueness index covered
    // trashed rows too, so it is rebuilt as a partial index; existing
    // databases cannot have trashed notebooks yet, so the rebuild is safe.
    Migration {
        version: 2,
        description: "add notebooks.is_trashed soft-delete for recursive trash",
        sql: "ALTER TABLE notebooks ADD COLUMN is_trashed INTEGER NOT NULL DEFAULT 0;\n\
              DROP INDEX IF EXISTS idx_notebooks_parent_name;\n\
              CREATE UNIQUE INDEX IF NOT EXISTS idx_notebooks_parent_name \
              ON notebooks(COALESCE(parent_id, -1), name) WHERE is_trashed = 0;\n\
              CREATE INDEX IF NOT EXISTS idx_notebooks_trashed ON notebooks(is_trashed);",
    },
    // v2 -> v3: attachments. Global resources table; no tombstones — orphaned
    // uuids are garbage-collected by reference scan after sync convergence.
    Migration {
        version: 3,
        description: "add resources table for Joplin-style attachments",
        sql: "CREATE TABLE IF NOT EXISTS resources (\n\
              uuid TEXT PRIMARY KEY,\n\
              filename TEXT NOT NULL,\n\
              mime TEXT NOT NULL DEFAULT '',\n\
              size INTEGER NOT NULL DEFAULT 0,\n\
              created_at TEXT NOT NULL DEFAULT (datetime('now')),\n\
              updated_at TEXT NOT NULL DEFAULT (datetime('now'))\n\
              );",
    },
];

/// Read the recorded schema version; databases without the version table
/// are treated as version 0.
pub(crate) async fn current_version(conn: &mut SqliteConnection) -> Result<i64> {
    let has_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
    )
    .fetch_one(&mut *conn)
    .await?;
    if has_table == 0 {
        return Ok(0);
    }
    let version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
        .fetch_one(&mut *conn)
        .await?;
    Ok(version)
}

/// Apply every migration newer than the database's recorded version.
///
/// Each step runs in its own transaction together with the version-row
/// insert, so a failed migration leaves both the schema and the version
/// record untouched. Re-running against an up-to-date database is a no-op.
/// Afterwards, indexes that depend on migrated columns (e.g.
/// `idx_notes_uuid`) are ensured.
///
/// Applied steps are recorded in the `schema_version` table (version +
/// applied_at) for later inspection; the data core itself stays silent.
pub(crate) async fn run(conn: &mut SqliteConnection) -> Result<()> {
    let version = current_version(conn).await?;
    for migration in MIGRATIONS.iter().filter(|m| m.version > version) {
        let mut tx = conn.begin().await?;
        let applied: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_version'",
        )
        .fetch_one(&mut *tx)
        .await?;
        if applied == 0 {
            sqlx::query(
                "CREATE TABLE schema_version (version INTEGER NOT NULL, applied_at TEXT NOT NULL DEFAULT (datetime('now')))",
            )
            .execute(&mut *tx)
            .await?;
        }
        sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO schema_version (version) VALUES (?)")
            .bind(migration.version)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    ensure_indexes(conn).await
}

/// Indexes that can only be created once their columns exist on every
/// database (fresh *and* migrated). Idempotent.
async fn ensure_indexes(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_uuid ON notes(uuid)")
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_notebooks_parent_name \
         ON notebooks(COALESCE(parent_id, -1), name) WHERE is_trashed = 0",
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_notebooks_trashed ON notebooks(is_trashed)")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Current schema version. Bump when adding a migration and append
    /// the matching step to [`MIGRATIONS`].
    const SCHEMA_VERSION: i64 = 3;

    #[test]
    fn migrations_are_ordered_and_versioned() {
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            assert_eq!(
                migration.version,
                index as i64 + 1,
                "migration {} must carry version index + 1",
                migration.description
            );
        }
        assert_eq!(
            MIGRATIONS.last().map(|m| m.version),
            Some(SCHEMA_VERSION),
            "SCHEMA_VERSION must match the last migration step"
        );
    }
}
