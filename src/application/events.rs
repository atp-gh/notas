//! Events emitted by core services.

use crate::core::model::{Note, NoteId, Notebook, SearchHit, Tag, TagCount};
use crate::core::repository::import::{ImportPreview, ImportStats};
use crate::core::sync::SyncStats;

/// Results emitted by persistence and synchronization services.
///
/// Error payloads are already rendered display strings: the frontend only
/// shows them (it never branches on the error kind), so a typed error
/// would be dead weight across the UI seam. The variants are otherwise
/// self-describing; per-variant docs would only restate the names.
#[derive(Debug, Clone)]
#[expect(missing_docs, reason = "variant names are self-describing")]
pub enum DbEvent {
    Notebooks(Vec<Notebook>),
    Tags(Vec<TagCount>),
    Notes(Vec<Note>),
    Trashed(Vec<Note>),
    NoteLoaded(Note),
    NoteCreated(Note),
    NoteSaved { id: NoteId },
    NoteTrashed { id: NoteId },
    NoteRestored { id: NoteId },
    NoteDeletedForever { id: NoteId },
    DataChanged,
    NoteTags(Vec<Tag>),
    SearchResults(Vec<SearchHit>),
    ExportDone(Result<usize, String>),
    ImportScanDone(Result<ImportPreview, String>),
    ImportDone(Result<ImportStats, String>),
    BackupDone(Result<(), String>),
    SyncDone(SyncStats),
    SyncFailed(String),
    Error(String),
}
