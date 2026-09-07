//! Value types shared by the pure sync planner and its executors.
//!
//! These types are the planner's input/output vocabulary:
//! [`LocalNote`] describes one local note, [`Sidecar`] is the remote
//! per-note metadata object, [`RemoteEntry`] is what the remote store
//! holds for one uuid, and [`SyncAction`] is one decision the executor
//! must carry out. See [`crate::core::sync`] for the planner's conflict,
//! deletion and tombstone rules.
//!
//! Remote sidecars are external input: [`Sidecar::validated`] enforces the
//! type invariants (non-empty uuid, timestamp shape) before a sidecar
//! reaches the planner, and [`Sidecar::normalized_tags`] gives the sorted,
//! de-duplicated tag list the planner's metadata comparisons rely on.

use serde::{Deserialize, Serialize};

/// Stable identifier for a note across devices. Not every string is a
/// valid sync id: it must be a non-empty ASCII string (historically a hex
/// uuid, but foreign/legacy ids are tolerated as long as they are
/// non-empty and contain no separators that would break storage paths).
///
/// The wire format stays a plain JSON string (serde is transparent), so
/// remote stores written by older versions keep working.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SyncUuid(String);

impl SyncUuid {
    /// Wrap a raw string. The executor only builds this from trusted
    /// sources (the local database, or keys it laid down itself), so
    /// invalid values are reported by [`Sidecar::validated`], not here.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The raw string form (used in storage keys and database lookups).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SyncUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for SyncUuid {
    fn from(raw: &str) -> Self {
        Self(raw.to_owned())
    }
}

impl From<String> for SyncUuid {
    fn from(raw: String) -> Self {
        Self(raw)
    }
}

/// Why a sidecar was rejected by [`Sidecar::validated`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SidecarError {
    /// The uuid is missing — the note cannot be identified across devices.
    #[error("sidecar has an empty sync uuid")]
    EmptyUuid,
    /// The uuid contains path separators and would break storage keys.
    #[error("sidecar uuid contains path separators")]
    UuidWithSeparators,
    /// The timestamp is not a SQLite `datetime('now')` string, so the
    /// planner's chronological string comparisons would be meaningless.
    #[error("sidecar timestamp is not `YYYY-MM-DD HH:MM:SS`")]
    BadTimestamp,
}

/// Parse a `YYYY-MM-DD HH:MM:SS` timestamp (length 19, digits and
/// separators in the right places). Leap-second `:60` and other range
/// checks are deliberately left to SQLite; the planner only needs a
/// consistent, comparable shape.
fn is_sqlite_datetime(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 19 {
        return false;
    }
    let digits = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    digits(&b[0..4])
        && b[4] == b'-'
        && digits(&b[5..7])
        && b[7] == b'-'
        && digits(&b[8..10])
        && b[10] == b' '
        && digits(&b[11..13])
        && b[13] == b':'
        && digits(&b[14..16])
        && b[16] == b':'
        && digits(&b[17..19])
}

/// Per-note metadata object stored next to every note on the remote store
/// (`meta/<uuid>.json`). It is the source of truth for everything except
/// the markdown body, which lives in `notes/<uuid>.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Sidecar {
    /// Stable cross-device identity.
    pub uuid: SyncUuid,
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

impl Sidecar {
    /// Check the invariants the planner relies on: a usable uuid and a
    /// comparable timestamp. Executors must call this before feeding a
    /// parsed sidecar into the remote index.
    ///
    /// # Errors
    ///
    /// Returns [`SidecarError`] describing the violated invariant.
    pub fn validated(&self) -> Result<&Self, SidecarError> {
        let uuid = self.uuid.as_str();
        if uuid.is_empty() {
            return Err(SidecarError::EmptyUuid);
        }
        if uuid.contains('/') || uuid.contains('\\') {
            return Err(SidecarError::UuidWithSeparators);
        }
        if !is_sqlite_datetime(&self.updated_at) {
            return Err(SidecarError::BadTimestamp);
        }
        Ok(self)
    }

    /// The tag list in canonical form: sorted, de-duplicated, with empty
    /// names removed. The planner compares metadata through this so tag
    /// ordering differences never look like content changes.
    #[must_use]
    pub fn normalized_tags(&self) -> Vec<String> {
        normalize_tags(&self.tags)
    }
}

/// Canonical tag-list form: trim, drop empties, sort, de-duplicate.
#[must_use]
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::with_capacity(tags.len());
    for tag in tags {
        let tag = tag.trim();
        if !tag.is_empty() && !seen.iter().any(|s| s == tag) {
            seen.push(tag.to_owned());
        }
    }
    seen.sort();
    seen
}

/// One note as seen from the local database, with everything the sidecar
/// needs to describe it on the remote store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalNote {
    /// Stable cross-device identity.
    pub uuid: SyncUuid,
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

impl LocalNote {
    /// The tag list in canonical form (see [`normalize_tags`]).
    #[must_use]
    pub fn normalized_tags(&self) -> Vec<String> {
        normalize_tags(&self.tags)
    }
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
        uuid: SyncUuid,
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
        uuid: SyncUuid,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sidecar(uuid: &str, updated_at: &str, tags: &[&str]) -> Sidecar {
        Sidecar {
            uuid: uuid.into(),
            title: "t".into(),
            notebook: None,
            tags: tags.iter().map(|s| (*s).to_owned()).collect(),
            trashed: false,
            deleted: false,
            updated_at: updated_at.into(),
            content_hash: String::new(),
        }
    }

    #[test]
    fn valid_sidecar_passes_validation() {
        let s = sidecar(
            "01234567-89ab-4cde-8f01-23456789abcd",
            "2026-01-01 10:00:00",
            &[],
        );
        assert!(s.validated().is_ok());
    }

    #[test]
    fn empty_uuid_is_rejected() {
        let s = sidecar("", "2026-01-01 10:00:00", &[]);
        assert_eq!(s.validated().unwrap_err(), SidecarError::EmptyUuid);
    }

    #[test]
    fn uuid_with_path_separators_is_rejected() {
        let s = sidecar("a/b", "2026-01-01 10:00:00", &[]);
        assert_eq!(s.validated().unwrap_err(), SidecarError::UuidWithSeparators);
    }

    #[test]
    fn malformed_timestamps_are_rejected() {
        for bad in [
            "",
            "2026-01-01",
            "2026-01-01T10:00:00",
            "2026-1-1 10:00:00",
            "not a timestamp",
            "2026-01-01  10:00:00",
        ] {
            let s = sidecar("u1", bad, &[]);
            assert_eq!(
                s.validated().unwrap_err(),
                SidecarError::BadTimestamp,
                "{bad}"
            );
        }
    }

    #[test]
    fn normalized_tags_sort_dedup_and_drop_empties() {
        let s = sidecar(
            "u1",
            "2026-01-01 10:00:00",
            &["b", " a ", "b", "", "c", "a"],
        );
        assert_eq!(s.normalized_tags(), vec!["a", "b", "c"]);
    }

    #[test]
    fn local_note_tags_normalize_the_same_way() {
        let n = LocalNote {
            uuid: "u1".into(),
            title: "t".into(),
            content: String::new(),
            is_trashed: false,
            updated_at: "2026-01-01 10:00:00".into(),
            notebook: None,
            tags: vec!["z".into(), "a".into(), "z".into()],
        };
        assert_eq!(n.normalized_tags(), vec!["a", "z"]);
    }
}
