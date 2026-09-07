//! Repository facade: every SQLite read/write behind one concrete type.
//!
//! The repository wraps the connection pool and groups operations by
//! aggregate (notes, notebooks, tags, sync state). It is a concrete struct,
//! not a trait: there is exactly one storage engine today, and a premature
//! `dyn Repository` seam would add indirection without value. All methods
//! are async and take `&self`; they never block the caller's thread.
//!
//! Transaction boundaries live here too: multi-step writes
//! (remote-note application, tag replacement, note deletion with its
//! tombstone) run inside a single SQLite transaction so a mid-way failure
//! cannot leave partially applied state behind.

use std::path::Path;

use sqlx::SqlitePool;

use crate::core::error::Result;
use crate::core::model::{Note, NoteId, Notebook, NotebookId, SearchHit, Tag, TagCount, TagId};

pub(crate) mod backup;
pub(crate) mod export;
pub(crate) mod notebooks;
pub(crate) mod notes;
pub(crate) mod search;
pub(crate) mod sync_state;
pub(crate) mod tags;

/// Concrete data-access object over the SQLite pool.
#[derive(Debug, Clone)]
pub struct Repository {
    /// Shared sqlx connection pool.
    pool: SqlitePool,
}

impl Repository {
    /// Build a repository on top of an existing pool.
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Borrow the underlying pool (for callers not yet migrated).
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // ---------------------------------------------------------- notebooks

    /// Insert a notebook and return the created row.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the insert fails.
    pub async fn create_notebook(
        &self,
        parent_id: Option<NotebookId>,
        name: &str,
    ) -> Result<Notebook> {
        notebooks::create(&self.pool, parent_id, name).await
    }

    /// Rename a notebook and bump its `updated_at`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::NotebookNotFound`] when the notebook
    /// does not exist, or [`crate::core::error::Error::Database`] on failure.
    pub async fn rename_notebook(&self, id: NotebookId, name: &str) -> Result<()> {
        notebooks::rename(&self.pool, id, name).await
    }

    /// Delete a notebook. Nested notebooks cascade; its notes become
    /// unfiled (`notebook_id` is set to NULL by the schema).
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::NotebookNotFound`] when the notebook
    /// does not exist, or [`crate::core::error::Error::Database`] on failure.
    pub async fn delete_notebook(&self, id: NotebookId) -> Result<()> {
        notebooks::delete(&self.pool, id).await
    }

    /// All notebooks, sorted by name.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn list_notebooks(&self) -> Result<Vec<Notebook>> {
        notebooks::list(&self.pool).await
    }

    // -------------------------------------------------------------- notes

    /// Insert a note (with the given title) and return the created row.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::NotebookNotFound`] when the target
    /// notebook does not exist, or [`crate::core::error::Error::Database`] on
    /// failure.
    pub async fn create_note(&self, notebook_id: Option<NotebookId>, title: &str) -> Result<Note> {
        notes::create(&self.pool, notebook_id, title).await
    }

    /// Fetch a single note by id, if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn get_note(&self, id: NoteId) -> Result<Option<Note>> {
        notes::get(&self.pool, id).await
    }

    /// Update title + content; the FTS index is kept in sync by trigger
    /// `notes_au`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::NoteNotFound`] when the note does not
    /// exist, or [`crate::core::error::Error::Database`] on failure.
    pub async fn update_note(&self, id: NoteId, title: &str, content: &str) -> Result<()> {
        notes::update(&self.pool, id, title, content).await
    }

    /// Move a note to the trash.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::NoteNotFound`] when the note does not
    /// exist, or [`crate::core::error::Error::Database`] on failure.
    pub async fn trash_note(&self, id: NoteId) -> Result<()> {
        notes::trash(&self.pool, id).await
    }

    /// Restore a trashed note.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::NoteNotFound`] when the note does not
    /// exist, or [`crate::core::error::Error::Database`] on failure.
    pub async fn restore_note(&self, id: NoteId) -> Result<()> {
        notes::restore(&self.pool, id).await
    }

    /// Physically delete a note (FTS row removed by trigger `notes_ad`).
    ///
    /// If the note had a sync uuid, a tombstone is recorded so the deletion
    /// propagates to other devices (they trash their copy) instead of the
    /// note resurrecting from the remote store on the next sync.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::NoteNotFound`] when the note does not
    /// exist, or [`crate::core::error::Error::Database`] on failure.
    pub async fn delete_note_forever(&self, id: NoteId) -> Result<()> {
        notes::delete_forever(&self.pool, id).await
    }

    /// Non-trashed notes of one notebook, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn list_notes(&self, notebook_id: NotebookId) -> Result<Vec<Note>> {
        notes::list_by_notebook(&self.pool, notebook_id).await
    }

    /// All non-trashed notes across every notebook, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn list_all_notes(&self) -> Result<Vec<Note>> {
        notes::list_all(&self.pool).await
    }

    /// All non-trashed notes carrying a given tag, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn list_notes_by_tag(&self, tag_id: TagId) -> Result<Vec<Note>> {
        notes::list_by_tag(&self.pool, tag_id).await
    }

    /// Notes whose notebook was deleted (notebook_id is NULL) but not
    /// trashed.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn list_unfiled_notes(&self) -> Result<Vec<Note>> {
        notes::list_unfiled(&self.pool).await
    }

    /// Trashed notes, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn list_trashed(&self) -> Result<Vec<Note>> {
        notes::list_trashed(&self.pool).await
    }

    // --------------------------------------------------------------- tags

    /// All tags with the number of notes using each, sorted by name.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn list_tags(&self) -> Result<Vec<TagCount>> {
        tags::list(&self.pool).await
    }

    /// The tags attached to one note, sorted by name.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn get_note_tags(&self, note_id: NoteId) -> Result<Vec<Tag>> {
        tags::get_for_note(&self.pool, note_id).await
    }

    /// Replace the tag set of a note. Tags are created on demand.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::NoteNotFound`] when the note does not
    /// exist, or [`crate::core::error::Error::Database`] on failure.
    pub async fn set_note_tags(&self, note_id: NoteId, names: &[String]) -> Result<()> {
        tags::set_for_note(&self.pool, note_id, names).await
    }

    /// Rename a tag. The `tags.name` UNIQUE constraint rejects colliding
    /// names.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::TagNotFound`] when the tag does not
    /// exist, or [`crate::core::error::Error::Database`] on failure.
    pub async fn rename_tag(&self, id: TagId, name: &str) -> Result<()> {
        tags::rename(&self.pool, id, name).await
    }

    /// Delete a tag; its `note_tags` links are removed by CASCADE.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::TagNotFound`] when the tag does not
    /// exist, or [`crate::core::error::Error::Database`] on failure.
    pub async fn delete_tag(&self, id: TagId) -> Result<()> {
        tags::delete(&self.pool, id).await
    }

    // -------------------------------------------------------------- sync

    /// Assign a uuid to every note that does not have one yet (notes
    /// created before the sync feature, or before the first sync). Returns
    /// the number of uuids assigned. Idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the update fails.
    pub async fn ensure_note_uuids(&self) -> Result<usize> {
        sync_state::ensure_note_uuids(&self.pool).await
    }

    /// Build the full local index the planner needs: every note (trashed
    /// included) with an assigned uuid, notebook path and tag names.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when a query fails.
    pub async fn sync_local_index(&self) -> Result<Vec<crate::core::sync::LocalNote>> {
        sync_state::local_index(&self.pool).await
    }

    /// Every locally recorded tombstone: `(uuid, deleted_at)` pairs for
    /// notes permanently deleted on this device.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn list_tombstones(&self) -> Result<Vec<(crate::core::sync::SyncUuid, String)>> {
        sync_state::list_tombstones(&self.pool).await
    }

    /// Apply a remote note locally: create it if the uuid is unknown,
    /// update it otherwise. The remote `updated_at` is preserved verbatim
    /// so the next sync sees an identical pair instead of re-uploading.
    ///
    /// Notebook resolution, the note upsert and the tag replacement run in
    /// one transaction: a mid-way failure rolls the whole remote
    /// application back instead of leaving a half-applied note behind.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] on SQL failures or
    /// [`crate::core::error::Error::InvalidInput`] when the remote note vanished
    /// mid-apply.
    pub async fn apply_remote_note(
        &self,
        sidecar: &crate::core::sync::Sidecar,
        content: &str,
    ) -> Result<()> {
        sync_state::apply_remote_note(&self.pool, sidecar, content).await
    }

    /// Create a local copy of the remote version of a note that collided
    /// with the local version at the same timestamp. The copy gets a fresh
    /// uuid (it will upload as a new note next sync), a `(conflict copy)`
    /// title suffix, and the remote body — so both texts survive on both
    /// devices.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] on SQL failures.
    pub async fn create_conflict_copy(
        &self,
        sidecar: &crate::core::sync::Sidecar,
        content: &str,
    ) -> Result<()> {
        sync_state::create_conflict_copy(&self.pool, sidecar, content).await
    }

    /// Move a note to the trash without bumping its `updated_at`, so a
    /// tombstone reaction doesn't make the local copy look newer than the
    /// tombstone on the next sync.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] on SQL failures.
    pub async fn trash_note_by_uuid_no_bump(
        &self,
        uuid: &crate::core::sync::SyncUuid,
    ) -> Result<()> {
        sync_state::trash_note_by_uuid_no_bump(&self.pool, uuid.as_str()).await
    }

    // ------------------------------------------------------------- search

    /// Full-text search over title + content, newest first, capped at 200
    /// hits.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Database`] when the query fails.
    pub async fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        search::run(&self.pool, query).await
    }

    // ------------------------------------------------------- export/backup

    /// Export every non-trashed note as `.md` files, grouped into notebook
    /// subdirectories. Returns the number of notes written.
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Io`] on filesystem failures and
    /// [`crate::core::error::Error::Database`] on SQL failures.
    pub async fn export_markdown(&self, out_dir: &Path) -> Result<usize> {
        export::run(&self.pool, out_dir).await
    }

    /// Create a consistent snapshot of the database at `dest` using
    /// `VACUUM INTO` (SQLite >= 3.27).
    ///
    /// # Errors
    ///
    /// Returns [`crate::core::error::Error::Io`] on filesystem failures and
    /// [`crate::core::error::Error::Database`] on SQL failures.
    pub async fn backup(&self, dest: &Path) -> Result<()> {
        backup::run(&self.pool, dest).await
    }
}
