//! Events emitted by core services.

use crate::domain_notes::{Note, Notebook, SearchHit, Tag, TagCount};
use crate::sync::SyncStats;

/// Results emitted by persistence and synchronization services.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub enum DbEvent {
    Notebooks(Vec<Notebook>),
    Tags(Vec<TagCount>),
    Notes(Vec<Note>),
    Trashed(Vec<Note>),
    NoteLoaded(Note),
    NoteCreated(Note),
    NoteSaved { id: i64 },
    NoteTrashed { id: i64 },
    NoteRestored { id: i64 },
    NoteDeletedForever { id: i64 },
    DataChanged,
    NoteTags(Vec<Tag>),
    SearchResults(Vec<SearchHit>),
    ExportDone(Result<usize, String>),
    BackupDone(Result<(), String>),
    SyncDone(SyncStats),
    SyncFailed(String),
    Error(String),
}
