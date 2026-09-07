//! Core data integration tests: schema upgrades and start-up invariants.
//!
//! These tests exercise the database lifecycle the way a user's machine
//! does: opening an existing database file, upgrading it, and restarting
//! against the upgraded file.

use notas::core::database;
use sqlx::{Row, SqlitePool};

/// A v0.1-era database: the schema *before* the sync feature added
/// `notes.uuid` and before the `schema_version` table existed.
async fn create_legacy_v0_pool(path: &std::path::Path) -> SqlitePool {
    let pool = database::connect(path).await.unwrap();
    // Roll the freshly created database back to the legacy shape: drop the
    // version table and rebuild `notes` without the uuid column (works on
    // every SQLite build, unlike DROP COLUMN).
    sqlx::query("DROP TABLE schema_version")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP INDEX IF EXISTS idx_notes_uuid")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE notes_legacy (\
             id          INTEGER PRIMARY KEY AUTOINCREMENT,\
             notebook_id INTEGER REFERENCES notebooks(id) ON DELETE SET NULL,\
             title       TEXT NOT NULL DEFAULT '',\
             content     TEXT NOT NULL DEFAULT '',\
             is_trashed  INTEGER NOT NULL DEFAULT 0,\
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),\
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))\
         );\
         INSERT INTO notes_legacy SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at FROM notes;\
         DROP TABLE notes;\
         ALTER TABLE notes_legacy RENAME TO notes;\
         DROP TABLE notes_fts;\
         DROP TRIGGER IF EXISTS notes_ai;\
         DROP TRIGGER IF EXISTS notes_ad;\
         DROP TRIGGER IF EXISTS notes_au;",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn legacy_database_without_uuid_column_is_upgraded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.db");

    {
        let pool = create_legacy_v0_pool(&path).await;
        // Seed a note the way the old version would have.
        sqlx::query("INSERT INTO notes (title, content) VALUES ('old note', 'old body')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    // Re-open with the current code: the upgrade must add uuid, index and
    // the version row without losing data.
    let pool = database::connect(&path).await.unwrap();
    let version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 1, "legacy database must be upgraded to v1");

    let has_uuid = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'uuid'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(has_uuid, 1, "uuid column must be added");

    let title: String = sqlx::query_scalar("SELECT title FROM notes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "old note", "existing data must survive the upgrade");
    pool.close().await;
}

#[tokio::test]
async fn repeated_startup_does_not_modify_schema_or_reapply_migrations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.db");

    let pool = database::connect(&path).await.unwrap();
    pool.close().await;

    // Snapshot the schema fingerprint after the first start.
    async fn fingerprint(pool: &SqlitePool) -> String {
        let mut rows = sqlx::query("SELECT type, name FROM sqlite_master ORDER BY type, name")
            .fetch_all(pool)
            .await
            .unwrap();
        rows.sort_by_key(|r| r.get::<String, _>("name"));
        let mut text = String::new();
        for row in rows {
            text.push_str(&row.get::<String, _>("type"));
            text.push(' ');
            text.push_str(&row.get::<String, _>("name"));
            text.push('\n');
        }
        text
    }

    let pool = database::connect(&path).await.unwrap();
    let after_first = fingerprint(&pool).await;
    let version_first: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .fetch_one(&pool)
            .await
            .unwrap();
    pool.close().await;

    // Second start: schema must be byte-identical, version unchanged.
    let pool = database::connect(&path).await.unwrap();
    let after_second = fingerprint(&pool).await;
    let version_second: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_first, after_second,
        "repeated startup must not alter the schema"
    );
    assert_eq!(version_first, version_second);
    pool.close().await;
}

#[tokio::test]
async fn tag_lookup_index_exists_after_connect() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database::connect(dir.path().join("a.db")).await.unwrap();

    // The tag-side membership index is part of the canonical schema, so
    // tag lookups never degrade to a full scan of note_tags.
    let index: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type = 'index' AND name = 'idx_note_tags_tag'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(index, 1);
    pool.close().await;
}

#[tokio::test]
async fn duplicate_notebook_names_under_one_parent_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database::connect(dir.path().join("a.db")).await.unwrap();

    sqlx::query("INSERT INTO notebooks (name) VALUES ('Work')")
        .execute(&pool)
        .await
        .unwrap();
    let second = sqlx::query("INSERT INTO notebooks (name) VALUES ('Work')")
        .execute(&pool)
        .await;
    assert!(
        second.is_err(),
        "same-name siblings must violate the unique index"
    );

    // Siblings under *different* parents may share a name.
    sqlx::query("INSERT INTO notebooks (name) VALUES ('Outer')")
        .execute(&pool)
        .await
        .unwrap();
    let outer: i64 = sqlx::query_scalar("SELECT id FROM notebooks WHERE name = 'Outer'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let nested = sqlx::query("INSERT INTO notebooks (parent_id, name) VALUES (?, 'Work')")
        .bind(outer)
        .execute(&pool)
        .await;
    assert!(nested.is_ok(), "same name under a different parent is fine");
    pool.close().await;
}
