//! Events emitted by core services.

use crate::core::model::{Note, NoteId, Notebook, SearchHit, Tag, TagCount};
use crate::core::sync::SyncStats;

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
    NoteSaved { id: NoteId },
    NoteTrashed { id: NoteId },
    NoteRestored { id: NoteId },
    NoteDeletedForever { id: NoteId },
    DataChanged,
    NoteTags(Vec<Tag>),
    SearchResults(Vec<SearchHit>),
    ExportDone(Result<usize, String>),
    BackupDone(Result<(), String>),
    SyncDone(SyncStats),
    SyncFailed(String),
    Error(String),
}
