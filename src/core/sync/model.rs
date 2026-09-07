//! Value types shared by the pure sync planner and its executors.
//!
//! These types are the planner's input/output vocabulary:
//! [`LocalNote`] describes one local note, [`Sidecar`] is the remote
//! per-note metadata object, [`RemoteEntry`] is what the remote store
//! holds for one uuid, and [`SyncAction`] is one decision the executor
//! must carry out. See [`crate::core::sync`] for the planner's conflict,
//! deletion and tombstone rules.

use serde::{Deserialize, Serialize};

/// One note as seen from the local database, with everything the sidecar
/// needs to describe it on the remote store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalNote {
    /// Stable cross-device identity.
    pub uuid: String,
    /// Display title.
    pub title: String,
    /// Markdown body.
    pub content: String,
    /// Whether the note sits in the trash.
    pub is_trashed: bool,
    /// Last modification time (`YYYY-MM-DD HH:MM:SS` UTC).
    pub updated_at: String,
    /// Notebook path (`"Parent/Child"`), or `None` for unfiled notes.
    pub notebook: Option<String>,
    /// Tag names attached to the note.
    pub tags: Vec<String>,
}

/// Per-note metadata object stored next to every note on the remote store
/// (`meta/<uuid>.json`). It is the source of truth for everything except
/// the markdown body, which lives in `notes/<uuid>.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Sidecar {
    /// Stable cross-device identity.
    pub uuid: String,
    /// Display title at upload time.
    pub title: String,
    /// Notebook path at upload time (`"Parent/Child"`), or `None`.
    pub notebook: Option<String>,
    /// Tag names at upload time.
    pub tags: Vec<String>,
    /// Whether the note was in the trash at upload time.
    pub trashed: bool,
    /// `true` for tombstones: the note was permanently deleted on the
    /// device that uploaded this sidecar. Tombstones carry no content.
    #[serde(default)]
    pub deleted: bool,
    /// Last modification time (`YYYY-MM-DD HH:MM:SS` UTC).
    pub updated_at: String,
    /// Hash of the markdown content so the planner can detect a content
    /// change without downloading the body. FNV-1a, not cryptographic.
    pub content_hash: String,
}

/// What the remote store holds for one uuid.
#[derive(Debug, Clone, Default)]
pub struct RemoteEntry {
    /// Parsed sidecar, if `meta/<uuid>.json` exists.
    pub sidecar: Option<Sidecar>,
    /// Whether `notes/<uuid>.md` exists.
    pub has_md: bool,
}

/// A decision the executor must carry out to converge local and remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// Upload this note's markdown + sidecar (new note, or the local copy
    /// is newer / the deterministic metadata winner).
    Upload {
        /// The note to upload, as seen locally.
        note: LocalNote,
    },
    /// Download the remote note and apply it locally; the executor fetches
    /// the markdown body itself.
    Download {
        /// The remote metadata to apply.
        sidecar: Sidecar,
    },
    /// Move the local note to the trash *without* bumping its timestamp
    /// (a tombstone reaction: it was permanently deleted on another
    /// device, and its timestamp is older than the tombstone).
    TrashLocal {
        /// The uuid of the local note to trash.
        uuid: String,
    },
    /// Keep the local note as-is and create a local copy of the remote
    /// version (new uuid, title suffixed `(conflict copy)`). Both texts
    /// are preserved on both devices.
    ConflictCopy {
        /// The remote metadata to copy.
        sidecar: Sidecar,
    },
    /// Upload a tombstone sidecar for a note permanently deleted locally.
    UploadTombstone {
        /// The uuid of the deleted note.
        uuid: String,
        /// When the note was deleted locally (`YYYY-MM-DD HH:MM:SS` UTC).
        deleted_at: String,
    },
}

/// Summary of one completed synchronization run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStats {
    /// Notes uploaded, including tombstones.
    pub uploaded: usize,
    /// Notes downloaded and applied locally.
    pub downloaded: usize,
    /// Local notes moved to the trash by a remote tombstone.
    pub trashed: usize,
    /// Conflict copies created from remote content.
    pub conflicts: usize,
    /// Local timestamp used for display and persistence.
    pub last_synced_at: String,
}
