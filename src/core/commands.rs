//! Commands submitted to core services.

use std::path::PathBuf;

use crate::core::config::SyncSettings;

/// Commands submitted to the persistence and synchronization service.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum DbCommand {
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

/// Semantic commands emitted by a frontend.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub enum AppCommand {
    Db(crate::core::events::DbEvent),
    SelectView(super::navigation::ViewId),
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
    ThemeChanged(crate::core::config::ThemeMode),
    ToggleLineNumbers(bool),
    ToggleStatusBar(bool),
    SyncNow,
    SyncTypeChanged(crate::core::config::SyncType),
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
