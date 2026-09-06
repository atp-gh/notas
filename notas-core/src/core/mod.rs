//! Application-level contracts shared by every frontend.

pub mod state;

use std::path::PathBuf;

use crate::config::SyncSettings;
use crate::config::{SyncType, ThemeMode};
use crate::models::{Note, Notebook, SearchHit, Tag, TagCount};
use crate::sync::SyncStats;

pub use crate::ui::{ViewId, ViewMode};

/// Commands submitted to the persistence and synchronization service.
#[derive(Debug)]
#[expect(
    missing_docs,
    reason = "command variants are self-describing and documented by the protocol"
)]
pub enum DbMsg {
    LoadNotebooks,
    LoadTags,
    LoadNotes(i64),
    LoadUnfiled,
    LoadAll,
    LoadTrashed,
    LoadByTag(i64),
    LoadNote(i64),
    CreateNote(Option<i64>),
    UpdateNote {
        id: i64,
        title: String,
        content: String,
    },
    TrashNote(i64),
    RestoreNote(i64),
    DeleteForever(i64),
    CreateNotebook {
        parent: Option<i64>,
        name: String,
    },
    RenameNotebook {
        id: i64,
        name: String,
    },
    DeleteNotebook(i64),
    RenameTag {
        id: i64,
        name: String,
    },
    DeleteTag(i64),
    SetTags {
        note_id: i64,
        names: Vec<String>,
    },
    LoadNoteTags(i64),
    Search(String),
    ExportMarkdown(PathBuf),
    Backup(PathBuf),
    SyncNow(Box<SyncSettings>),
}

/// Results emitted by the persistence and synchronization service.
#[derive(Debug, Clone)]
#[expect(
    missing_docs,
    reason = "event variants are self-describing and documented by the protocol"
)]
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

/// Semantic events emitted by any frontend.
#[derive(Debug, Clone)]
#[expect(
    missing_docs,
    reason = "event variants are self-describing and documented by the protocol"
)]
pub enum AppMsg {
    Db(DbEvent),
    SelectView(ViewId),
    SelectNotebook(i64),
    SelectTag(Option<i64>),
    SelectNote(i64),
    SearchChanged(String),
    NewNote,
    NewNotebook(String),
    RenameNotebook { id: i64, name: String },
    DeleteNotebook(i64),
    RenameTag { id: i64, name: String },
    DeleteTag(i64),
    TrashNote,
    RestoreNote,
    DeleteForever,
    DeleteForeverConfirmed,
    SaveNote,
    TitleChanged,
    ContentChanged,
    TogglePreview,
    FocusSearch,
    FocusFind,
    FindChanged(String),
    FindNext,
    FindPrev,
    ReplaceAll(String),
    TagsEdited(Vec<String>),
    ExportMarkdown,
    ExportTo(PathBuf),
    BackupNow,
    BackupTo(PathBuf),
    DialogSave,
    DialogDiscard,
    DialogCancel,
    CloseRequested,
    OpenSettings,
    ThemeChanged(ThemeMode),
    ToggleLineNumbers(bool),
    ToggleStatusBar(bool),
    SyncNow,
    SyncTypeChanged(SyncType),
    SyncEndpointChanged(String),
    SyncRegionChanged(String),
    SyncBucketChanged(String),
    SyncPrefixChanged(String),
    SyncAccessKeyChanged(String),
    SyncSecretKeyChanged(String),
    SyncUrlChanged(String),
    SyncDirectoryChanged(String),
    SyncUsernameChanged(String),
    SyncPasswordChanged(String),
    SyncInsecureTlsChanged(bool),
    SyncEncryptionEnabledChanged(bool),
    SyncEncryptionPasswordChanged(String),
}
