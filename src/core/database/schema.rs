//! SQLite schema definition.
//!
//! The canonical DDL for a fresh database: tables, indexes, the FTS5
//! index, its sync triggers, and the schema-version bookkeeping table.
//! Upgrades of databases created by older versions are handled by
//! [`super::migrations`], which is why the hand-written `notes.uuid`
//! special case lives there and not in this DDL.

/// Idempotent DDL: tables, indexes, the FTS5 index and its sync triggers.
/// Safe to run on every start-up.
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS notebooks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id  INTEGER REFERENCES notebooks(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS notes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    notebook_id INTEGER REFERENCES notebooks(id) ON DELETE SET NULL,
    title       TEXT NOT NULL DEFAULT '',
    content     TEXT NOT NULL DEFAULT '',
    is_trashed  INTEGER NOT NULL DEFAULT 0,
    -- Stable identity across devices, assigned by the sync engine
    -- (NULL until the first sync assigns one).
    uuid        TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS tags (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS note_tags (
    note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    tag_id  INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (note_id, tag_id)
);

CREATE INDEX IF NOT EXISTS idx_notes_notebook ON notes(notebook_id);
CREATE INDEX IF NOT EXISTS idx_notes_trashed  ON notes(is_trashed);
-- NOTE: the `notes.uuid` unique index is NOT created here: legacy databases
-- reach this DDL without the uuid column (the column is added by migration
-- v1), and `CREATE INDEX ... ON notes(uuid)` would fail. The index is
-- ensured after migrations run (see `migrations::ensure_indexes`).

-- Tombstones for the sync engine: uuids of notes permanently deleted on
-- this device. They prevent a deleted note from "resurrecting" from the
-- remote store on the next sync, and let other devices move their copy to
-- the trash instead of deleting it.
CREATE TABLE IF NOT EXISTS sync_tombstones (
    uuid       TEXT PRIMARY KEY,
    deleted_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- A notebook is unique among its siblings: concurrent syncs resolving the
-- same path must not create duplicate "Parent/Child" rows. NULL parents
-- (top-level notebooks) are distinct in SQLite unique indexes, which is
-- what we want: each NULL is a separate top-level slot keyed by name.
CREATE UNIQUE INDEX IF NOT EXISTS idx_notebooks_parent_name
    ON notebooks(COALESCE(parent_id, -1), name);

-- Version bookkeeping for the ordered migrations (see `migrations.rs`).
-- A database that has this table with no rows was created by an older
-- pre-versioning build and is migrated from version 0; a fresh database
-- gets the current version seeded right here, so the migration steps
-- (which pre-date versioning) never re-run against it.
CREATE TABLE IF NOT EXISTS schema_version (
    version    INTEGER NOT NULL,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);
INSERT INTO schema_version (version)
SELECT 1
WHERE NOT EXISTS (SELECT 1 FROM schema_version)
  AND EXISTS (SELECT 1 FROM pragma_table_info('notes') WHERE name = 'uuid');

-- Full-text search over title + content, external content keeps the index
-- in sync with the notes table via triggers.
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    title, content,
    content='notes', content_rowid='id',
    tokenize='unicode61'
);

CREATE TRIGGER IF NOT EXISTS notes_ai AFTER INSERT ON notes BEGIN
    INSERT INTO notes_fts(rowid, title, content) VALUES (new.id, new.title, new.content);
END;

CREATE TRIGGER IF NOT EXISTS notes_ad AFTER DELETE ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, title, content) VALUES ('delete', old.id, old.title, old.content);
END;

CREATE TRIGGER IF NOT EXISTS notes_au AFTER UPDATE ON notes BEGIN
    INSERT INTO notes_fts(notes_fts, rowid, title, content) VALUES ('delete', old.id, old.title, old.content);
    INSERT INTO notes_fts(rowid, title, content) VALUES (new.id, new.title, new.content);
END;
"#;
