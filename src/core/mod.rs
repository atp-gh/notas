//! Application-level contracts shared by every frontend.

#![deny(missing_docs)]

pub mod config;
pub mod error;
pub mod markdown;
pub mod notes;
pub mod search;
pub mod state;
pub mod storage;
pub mod ui;

pub use error::{Error, Result};

use std::path::PathBuf;

use self::config::{SyncSettings, SyncType, ThemeMode};
use self::notes::models::{Note, Notebook, SearchHit, Tag, TagCount};
use crate::sync::SyncStats;

pub use self::ui::{ViewId, ViewMode};

/// Commands submitted to the persistence and synchronization service.
#[derive(Debug)]
#[allow(missing_docs)]
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
    ExportDone(std::result::Result<usize, String>),
    BackupDone(std::result::Result<(), String>),
    SyncDone(SyncStats),
    SyncFailed(String),
    Error(String),
}

/// Semantic events emitted by any frontend.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
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
