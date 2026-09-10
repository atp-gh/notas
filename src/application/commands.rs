//! Commands submitted to core services.

use std::path::PathBuf;

use crate::application::config::SyncSettings;
use crate::application::events::DbEvent;
use crate::application::navigation::ViewId;
use crate::core::model::{NoteId, NotebookId, TagId};

/// Commands submitted to the persistence and synchronization service.
///
/// The variants are self-describing payloads (one per repository intent);
/// per-variant docs would only restate the names.
#[derive(Debug, Clone)]
#[expect(missing_docs, reason = "variant names are self-describing")]
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
    DuplicateNote {
        id: NoteId,
        target: Option<NotebookId>,
    },
    MoveNote {
        id: NoteId,
        target: Option<NotebookId>,
    },
    CreateNotebook {
        parent: Option<NotebookId>,
        name: String,
    },
    RenameNotebook {
        id: NotebookId,
        name: String,
    },
    DeleteNotebook(NotebookId),
    TrashNotebook(NotebookId),
    RestoreNotebook(NotebookId),
    MoveNotebook {
        id: NotebookId,
        target: Option<NotebookId>,
    },
    DuplicateNotebook {
        id: NotebookId,
        target: Option<NotebookId>,
    },
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
    ImportScan(PathBuf),
    ImportMarkdown(PathBuf),
    Backup(PathBuf),
    SyncNow(Box<SyncSettings>),
}

/// Editor pane layout: source-only, side-by-side live split, or
/// preview-only. Joplin-style tri-state; the preview-carrying modes render
/// asynchronously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    /// Source editor only.
    Source,
    /// Source on the left, live preview on the right.
    Split,
    /// Rendered preview only.
    Preview,
}

impl EditorMode {
    /// Next mode for the `Ctrl+E` cycle: Source → Split → Preview → Source.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Source => Self::Split,
            Self::Split => Self::Preview,
            Self::Preview => Self::Source,
        }
    }
}
/// Semantic commands emitted by a frontend.
///
/// Like [`DbCommand`], the variants are self-describing; docs would only
/// restate the names.
#[derive(Debug, Clone)]
#[expect(missing_docs, reason = "variant names are self-describing")]
pub enum AppCommand {
    Db(DbEvent),
    SelectView(ViewId),
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
    TrashNoteById(NoteId),
    RestoreNote,
    DeleteForever,
    DeleteForeverConfirmed,
    CopyNote(NoteId),
    CutNote(NoteId),
    CopyNotebook(NotebookId),
    CutNotebook(NotebookId),
    PasteToNotebook(Option<NotebookId>),
    PasteToNote(NoteId),
    TrashNotebook(NotebookId),
    RestoreNotebook(NotebookId),
    SaveNote,
    TitleChanged,
    ContentChanged,
    SetEditorMode(EditorMode),
    CycleEditorMode,
    FocusSearch,
    FocusFind,
    FindChanged(String),
    FindNext,
    FindPrev,
    ReplaceAll(String),
    TagsEdited(Vec<String>),
    ExportMarkdown,
    ExportTo(PathBuf),
    ImportMarkdown,
    ImportFrom(PathBuf),
    ImportConfirmed,
    BackupNow,
    BackupTo(PathBuf),
    DialogSave,
    DialogDiscard,
    DialogCancel,
    CloseRequested,
    OpenSettings,
    ThemeChanged(crate::application::config::ThemeMode),
    ToggleLineNumbers(bool),
    ToggleStatusBar(bool),
    SyncNow,
    SyncTypeChanged(crate::application::config::SyncType),
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
