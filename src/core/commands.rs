//! Commands submitted to core services.

use std::path::PathBuf;

use crate::core::config::SyncSettings;
use crate::core::model::{NoteId, NotebookId, TagId};

/// Commands submitted to the persistence and synchronization service.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum DbCommand {
    LoadNotebooks,
    LoadTags,
    LoadNotes(NotebookId),
    LoadUnfiled,
    LoadAll,
    LoadTrashed,
    LoadByTag(TagId),
    LoadNote(NoteId),
    CreateNote(Option<NotebookId>),
    UpdateNote {
        id: NoteId,
        title: String,
        content: String,
    },
    TrashNote(NoteId),
    RestoreNote(NoteId),
    DeleteForever(NoteId),
    CreateNotebook {
        parent: Option<NotebookId>,
        name: String,
    },
    RenameNotebook {
        id: NotebookId,
        name: String,
    },
    DeleteNotebook(NotebookId),
    RenameTag {
        id: TagId,
        name: String,
    },
    DeleteTag(TagId),
    SetTags {
        note_id: NoteId,
        names: Vec<String>,
    },
    LoadNoteTags(NoteId),
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
    SelectNotebook(NotebookId),
    SelectTag(Option<TagId>),
    SelectNote(NoteId),
    SearchChanged(String),
    NewNote,
    NewNotebook(String),
    RenameNotebook { id: NotebookId, name: String },
    DeleteNotebook(NotebookId),
    RenameTag { id: TagId, name: String },
    DeleteTag(TagId),
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
