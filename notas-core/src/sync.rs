//! Sync planning: pure, testable logic shared by every sync backend.
//!
//! The S3-specific I/O lives in the app crate; this module only decides
//! *what* needs to happen given a local index and a remote index. Keeping
//! it free of network and database code makes the conflict, deletion and
//! tombstone rules unit-testable (`cargo test -p notas-core`).
//!
//! ## Model
//!
//! Every note is identified across devices by a stable [`uuid`]; the local
//! SQLite `notes.id` is device-specific and never leaves the machine. Each
//! note exists on the remote store as a pair of objects:
//!
//! - `notes/<uuid>.md` — the markdown body;
//! - `meta/<uuid>.json` — a [`Sidecar`] carrying everything else.
//!
//! A sidecar with [`Sidecar::deleted`] set is a *tombstone*: the note was
//! permanently deleted on the device that uploaded it, so other devices
//! move their copy to the trash instead of letting it resurrect.
//!
//! ## Conflict rules
//!
//! - Newer [`LocalNote::updated_at`] wins (last-write-wins).
//! - Equal timestamps with *different* content: keep the local note and
//!   preserve the remote version as a conflict copy (new note, title
//!   suffixed with `(conflict copy)`), so no text is ever lost.
//! - Equal timestamps with equal content but different metadata (title,
//!   notebook, tags, trash state): adopt the deterministically smaller
//!   tuple on both devices, so the state converges instead of oscillating.
//!
//! Timestamps are SQLite `datetime('now')` strings (`YYYY-MM-DD HH:MM:SS`
//! UTC), which are zero-padded and therefore compare chronologically as
//! plain strings. They have one-second resolution, which is why ties need
//! explicit handling.

use std::collections::{HashMap, HashSet};

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

/// FNV-1a 64-bit hash of the markdown content, hex-encoded. Used only for
/// change detection (equal timestamps, different body); deliberately not
/// cryptographic.
pub fn content_hash(content: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Deterministic tiebreak key for the "same timestamp, same content,
/// different metadata" case: both devices must compute the same winner or
/// the state oscillates forever.
fn local_meta_key(note: &LocalNote) -> (&str, Option<&str>, &[String], bool) {
    (
        note.title.as_str(),
        note.notebook.as_deref(),
        &note.tags,
        note.is_trashed,
    )
}

fn sidecar_meta_key(sidecar: &Sidecar) -> (&str, Option<&str>, &[String], bool) {
    (
        sidecar.title.as_str(),
        sidecar.notebook.as_deref(),
        &sidecar.tags,
        sidecar.trashed,
    )
}

/// Compare the local database against the remote store and decide every
/// action needed to converge them.
///
/// - `local` — every note (including trashed ones) with an assigned uuid.
/// - `tombstones` — `(uuid, deleted_at)` pairs for notes permanently
///   deleted locally.
/// - `remote` — what the store holds, keyed by uuid.
pub fn plan_sync(
    local: &[LocalNote],
    tombstones: &[(String, String)],
    remote: &HashMap<String, RemoteEntry>,
) -> Vec<SyncAction> {
    let local_by_uuid: HashMap<&str, &LocalNote> =
        local.iter().map(|n| (n.uuid.as_str(), n)).collect();
    let tombstone_uuids: HashSet<&str> = tombstones.iter().map(|(uuid, _)| uuid.as_str()).collect();
    let mut actions = Vec::new();

    // --- local notes ------------------------------------------------------
    for note in local {
        // Nothing remote: first sync, or the remote copy is an orphan
        // md without a sidecar — upload the full note either way.
        let Some(entry) = remote.get(&note.uuid) else {
            actions.push(SyncAction::Upload { note: note.clone() });
            continue;
        };
        let Some(sidecar) = entry.sidecar.as_ref() else {
            actions.push(SyncAction::Upload { note: note.clone() });
            continue;
        };
        if sidecar.deleted {
            // A tombstone: the note was permanently deleted on the
            // device that uploaded it. Respect it unless the local
            // note was touched *after* the tombstone (an explicit
            // restore or edit, which resurrects the note).
            if note.is_trashed {
                // Already in the trash; don't fight the tombstone
                // by re-uploading.
            } else if sidecar.updated_at >= note.updated_at {
                actions.push(SyncAction::TrashLocal {
                    uuid: note.uuid.clone(),
                });
            } else {
                actions.push(SyncAction::Upload { note: note.clone() });
            }
        } else if sidecar.updated_at < note.updated_at {
            actions.push(SyncAction::Upload { note: note.clone() });
        } else if sidecar.updated_at > note.updated_at {
            if entry.has_md {
                actions.push(SyncAction::Download {
                    sidecar: sidecar.clone(),
                });
            }
            // sidecar newer but the body is missing: broken remote
            // state, leave the local copy alone.
        } else if sidecar.content_hash != content_hash(&note.content) {
            // Same second, different body: keep the local note and
            // preserve the remote version as a conflict copy.
            actions.push(SyncAction::ConflictCopy {
                sidecar: sidecar.clone(),
            });
        } else if sidecar_meta_key(sidecar) != local_meta_key(note) {
            // Same second, same body, different metadata: adopt the
            // deterministically smaller tuple on both devices.
            if sidecar_meta_key(sidecar) < local_meta_key(note) {
                actions.push(SyncAction::Download {
                    sidecar: sidecar.clone(),
                });
            } else {
                actions.push(SyncAction::Upload { note: note.clone() });
            }
        }
        // else: identical on both sides — nothing to do.
    }

    // --- local tombstones -------------------------------------------------
    for (uuid, deleted_at) in tombstones {
        match remote.get(uuid) {
            Some(entry) => match &entry.sidecar {
                // Already honored remotely.
                Some(s) if s.deleted => {}
                // Another device touched the note *after* our deletion
                // (resurrection): accept the newer remote version back.
                Some(s) if s.updated_at.as_str() > deleted_at.as_str() => {
                    actions.push(SyncAction::Download { sidecar: s.clone() });
                }
                // Stale remote copy: propagate the tombstone.
                _ => actions.push(SyncAction::UploadTombstone {
                    uuid: uuid.clone(),
                    deleted_at: deleted_at.clone(),
                }),
            },
            None => actions.push(SyncAction::UploadTombstone {
                uuid: uuid.clone(),
                deleted_at: deleted_at.clone(),
            }),
        }
    }

    // --- remote notes with no local counterpart ---------------------------
    for (uuid, entry) in remote {
        if local_by_uuid.contains_key(uuid.as_str()) || tombstone_uuids.contains(uuid.as_str()) {
            continue;
        }
        if let Some(sidecar) = &entry.sidecar
            && !sidecar.deleted
            && entry.has_md
        {
            actions.push(SyncAction::Download {
                sidecar: sidecar.clone(),
            });
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(uuid: &str, title: &str, content: &str, updated_at: &str) -> LocalNote {
        LocalNote {
            uuid: uuid.into(),
            title: title.into(),
            content: content.into(),
            is_trashed: false,
            updated_at: updated_at.into(),
            notebook: None,
            tags: Vec::new(),
        }
    }

    fn sidecar_of(n: &LocalNote) -> Sidecar {
        Sidecar {
            uuid: n.uuid.clone(),
            title: n.title.clone(),
            notebook: n.notebook.clone(),
            tags: n.tags.clone(),
            trashed: n.is_trashed,
            deleted: false,
            updated_at: n.updated_at.clone(),
            content_hash: content_hash(&n.content),
        }
    }

    fn remote_one(sidecar: Sidecar) -> HashMap<String, RemoteEntry> {
        let mut remote = HashMap::new();
        remote.insert(
            sidecar.uuid.clone(),
            RemoteEntry {
                sidecar: Some(sidecar),
                has_md: true,
            },
        );
        remote
    }

    #[test]
    fn new_local_note_uploads() {
        let local = [note("u1", "A", "hello", "2026-01-01 10:00:00")];
        let actions = plan_sync(&local, &[], &HashMap::new());
        assert_eq!(
            actions,
            vec![SyncAction::Upload {
                note: local[0].clone()
            }]
        );
    }

    #[test]
    fn new_remote_note_downloads() {
        let n = note("u1", "A", "hello", "2026-01-01 10:00:00");
        let remote = remote_one(sidecar_of(&n));
        let actions = plan_sync(&[], &[], &remote);
        assert_eq!(
            actions,
            vec![SyncAction::Download {
                sidecar: remote["u1"].sidecar.clone().unwrap()
            }]
        );
    }

    #[test]
    fn identical_notes_noop() {
        let local = [note("u1", "A", "hello", "2026-01-01 10:00:00")];
        let remote = remote_one(sidecar_of(&local[0]));
        assert!(plan_sync(&local, &[], &remote).is_empty());
    }

    #[test]
    fn local_newer_uploads() {
        let local = [note("u1", "A", "newer", "2026-01-02 10:00:00")];
        let mut older = sidecar_of(&note("u1", "A", "older", "2026-01-01 10:00:00"));
        older.content_hash = content_hash("older");
        let remote = remote_one(older);
        let actions = plan_sync(&local, &[], &remote);
        assert_eq!(
            actions,
            vec![SyncAction::Upload {
                note: local[0].clone()
            }]
        );
    }

    #[test]
    fn remote_newer_downloads() {
        let local = [note("u1", "A", "older", "2026-01-01 10:00:00")];
        let remote = remote_one(sidecar_of(&note("u1", "A", "newer", "2026-01-02 10:00:00")));
        let actions = plan_sync(&local, &[], &remote);
        assert_eq!(
            actions,
            vec![SyncAction::Download {
                sidecar: remote["u1"].sidecar.clone().unwrap()
            }]
        );
    }

    #[test]
    fn tie_with_different_content_makes_conflict_copy() {
        let local = [note("u1", "A", "local text", "2026-01-01 10:00:00")];
        let remote = remote_one(sidecar_of(&note(
            "u1",
            "A",
            "remote text",
            "2026-01-01 10:00:00",
        )));
        let actions = plan_sync(&local, &[], &remote);
        // No upload (would fight the tie forever) and no download; the
        // remote text is preserved as a conflict copy instead.
        assert_eq!(
            actions,
            vec![SyncAction::ConflictCopy {
                sidecar: remote["u1"].sidecar.clone().unwrap()
            }]
        );
    }

    #[test]
    fn tie_same_content_metadata_adopts_deterministic_winner() {
        // Local title sorts after the remote one -> remote wins, download.
        let local = [note("u1", "Zebra", "same", "2026-01-01 10:00:00")];
        let remote = remote_one(sidecar_of(&note(
            "u1",
            "Apple",
            "same",
            "2026-01-01 10:00:00",
        )));
        let actions = plan_sync(&local, &[], &remote);
        assert_eq!(
            actions,
            vec![SyncAction::Download {
                sidecar: remote["u1"].sidecar.clone().unwrap()
            }]
        );

        // Local title sorts before the remote one -> local wins, upload.
        let local = [note("u1", "Apple", "same", "2026-01-01 10:00:00")];
        let remote = remote_one(sidecar_of(&note(
            "u1",
            "Zebra",
            "same",
            "2026-01-01 10:00:00",
        )));
        let actions = plan_sync(&local, &[], &remote);
        assert_eq!(
            actions,
            vec![SyncAction::Upload {
                note: local[0].clone()
            }]
        );
    }

    #[test]
    fn tombstone_trashes_older_local_note_without_resurrecting() {
        let local = [note("u1", "A", "old", "2026-01-01 10:00:00")];
        let mut tomb = sidecar_of(&note("u1", "A", "old", "2026-01-01 10:00:00"));
        tomb.deleted = true;
        tomb.updated_at = "2026-01-03 10:00:00".into();
        let remote = remote_one(tomb);
        let actions = plan_sync(&local, &[], &remote);
        assert_eq!(actions, vec![SyncAction::TrashLocal { uuid: "u1".into() }]);
    }

    #[test]
    fn tombstone_yields_to_newer_local_edit() {
        // Local note was edited after the tombstone -> resurrect it.
        let local = [note("u1", "A", "edited after", "2026-01-04 10:00:00")];
        let mut tomb = sidecar_of(&note("u1", "A", "old", "2026-01-01 10:00:00"));
        tomb.deleted = true;
        tomb.updated_at = "2026-01-03 10:00:00".into();
        let remote = remote_one(tomb);
        let actions = plan_sync(&local, &[], &remote);
        assert_eq!(
            actions,
            vec![SyncAction::Upload {
                note: local[0].clone()
            }]
        );
    }

    #[test]
    fn trashed_local_note_ignores_tombstone() {
        let mut local = note("u1", "A", "old", "2026-01-01 10:00:00");
        local.is_trashed = true;
        let mut tomb = sidecar_of(&note("u1", "A", "old", "2026-01-01 10:00:00"));
        tomb.deleted = true;
        tomb.updated_at = "2026-01-03 10:00:00".into();
        let remote = remote_one(tomb);
        assert!(plan_sync(&[local], &[], &remote).is_empty());
    }

    #[test]
    fn local_tombstone_uploads_when_remote_stale() {
        let n = note("u1", "A", "old", "2026-01-01 10:00:00");
        let remote = remote_one(sidecar_of(&n));
        let actions = plan_sync(&[], &[("u1".into(), "2026-01-02 10:00:00".into())], &remote);
        assert_eq!(
            actions,
            vec![SyncAction::UploadTombstone {
                uuid: "u1".into(),
                deleted_at: "2026-01-02 10:00:00".into()
            }]
        );
    }

    #[test]
    fn local_tombstone_accepts_resurrected_remote() {
        // Another device edited the note after our deletion -> accept it.
        let n = note("u1", "A", "resurrected", "2026-01-04 10:00:00");
        let remote = remote_one(sidecar_of(&n));
        let actions = plan_sync(&[], &[("u1".into(), "2026-01-02 10:00:00".into())], &remote);
        assert_eq!(
            actions,
            vec![SyncAction::Download {
                sidecar: remote["u1"].sidecar.clone().unwrap()
            }]
        );
    }

    #[test]
    fn local_tombstone_noop_when_remote_already_tombstoned() {
        let mut tomb = sidecar_of(&note("u1", "A", "old", "2026-01-01 10:00:00"));
        tomb.deleted = true;
        tomb.updated_at = "2026-01-03 10:00:00".into();
        let remote = remote_one(tomb);
        assert!(plan_sync(&[], &[("u1".into(), "2026-01-02 10:00:00".into())], &remote).is_empty());
    }

    #[test]
    fn remote_tombstone_for_unknown_note_is_noop() {
        let mut tomb = sidecar_of(&note("u1", "A", "old", "2026-01-01 10:00:00"));
        tomb.deleted = true;
        let remote = remote_one(tomb);
        assert!(plan_sync(&[], &[], &remote).is_empty());
    }

    #[test]
    fn remote_trashed_note_downloads_as_trashed() {
        let mut n = note("u1", "A", "body", "2026-01-01 10:00:00");
        n.is_trashed = true;
        let remote = remote_one(sidecar_of(&n));
        let actions = plan_sync(&[], &[], &remote);
        let SyncAction::Download { sidecar } = &actions[0] else {
            panic!("expected Download");
        };
        assert!(sidecar.trashed);
    }

    #[test]
    fn content_hash_changes_with_content() {
        assert_ne!(content_hash("a"), content_hash("b"));
        assert_eq!(content_hash("a"), content_hash("a"));
    }
}
