//! Deterministic sync planning: conflict, deletion and tombstone decisions.
//!
//! This module only decides *what* needs to happen given a local index and
//! a remote index; the transport executor (see [`crate::sync::executor`])
//! turns the plan into actual store and database operations. Keeping the
//! planner free of network, database, crypto and time code makes the
//! conflict, deletion and tombstone rules unit-testable.
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

use super::model::{LocalNote, RemoteEntry, Sidecar, SyncAction, SyncUuid};

/// FNV-1a 64-bit hash of the markdown content, hex-encoded. Used only for
/// change detection (equal timestamps, different body); deliberately not
/// cryptographic.
///
/// # Examples
///
/// ```
/// use notas::core::sync::content_hash;
///
/// assert_eq!(content_hash("hello"), content_hash("hello"));
/// assert_ne!(content_hash("hello"), content_hash("hello!"));
/// assert_eq!(content_hash(""), "cbf29ce484222325");
/// ```
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
/// the state oscillates forever. Tags compare in canonical form (sorted,
/// de-duplicated) so mere ordering differences never register as metadata
/// changes.
fn local_meta_key(note: &LocalNote) -> (&str, Option<&str>, Vec<String>, bool) {
    (
        note.title.as_str(),
        note.notebook.as_deref(),
        note.normalized_tags(),
        note.is_trashed,
    )
}

fn sidecar_meta_key(sidecar: &Sidecar) -> (&str, Option<&str>, Vec<String>, bool) {
    (
        sidecar.title.as_str(),
        sidecar.notebook.as_deref(),
        sidecar.normalized_tags(),
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
///
/// # Examples
///
/// A local note with no remote counterpart uploads; an identical pair
/// converges to nothing:
///
/// ```
/// use std::collections::HashMap;
/// use notas::core::sync::{LocalNote, RemoteEntry, Sidecar, SyncUuid, plan_sync};
///
/// let local = [LocalNote {
///     uuid: SyncUuid::new("a"),
///     title: "A".into(),
///     content: "hello".into(),
///     is_trashed: false,
///     updated_at: "2026-01-01 10:00:00".into(),
///     notebook: None,
///     tags: Vec::new(),
/// }];
///
/// let mut remote: HashMap<SyncUuid, RemoteEntry> = HashMap::new();
/// remote.insert(
///     SyncUuid::new("a"),
///     RemoteEntry { sidecar: None, has_md: false },
/// );
/// assert_eq!(plan_sync(&local, &[], &remote).len(), 1);
///
/// // With a matching sidecar for the same body, nothing needs doing.
/// let sidecar = Sidecar {
///     uuid: SyncUuid::new("a"),
///     title: "A".into(),
///     notebook: None,
///     tags: Vec::new(),
///     trashed: false,
///     deleted: false,
///     updated_at: "2026-01-01 10:00:00".into(),
///     content_hash: notas::core::sync::content_hash("hello"),
/// };
/// remote.insert(
///     SyncUuid::new("a"),
///     RemoteEntry { sidecar: Some(sidecar), has_md: true },
/// );
/// assert!(plan_sync(&local, &[], &remote).is_empty());
/// ```
pub fn plan_sync(
    local: &[LocalNote],
    tombstones: &[(SyncUuid, String)],
    remote: &HashMap<SyncUuid, RemoteEntry>,
) -> Vec<SyncAction> {
    let local_by_uuid: HashMap<&SyncUuid, &LocalNote> =
        local.iter().map(|n| (&n.uuid, n)).collect();
    let tombstone_uuids: HashSet<&SyncUuid> = tombstones.iter().map(|(uuid, _)| uuid).collect();
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
    // The remote index is a HashMap: sort by uuid so the action order —
    // and therefore the order the executor creates notes in — does not
    // depend on hash iteration order.
    let mut remote_uuids: Vec<&SyncUuid> = remote.keys().collect();
    remote_uuids.sort_unstable();
    for uuid in remote_uuids {
        let entry = &remote[uuid];
        if local_by_uuid.contains_key(uuid) || tombstone_uuids.contains(uuid) {
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

    fn remote_one(sidecar: Sidecar) -> HashMap<SyncUuid, RemoteEntry> {
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
                sidecar: remote[&SyncUuid::new("u1")].sidecar.clone().unwrap()
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
                sidecar: remote[&SyncUuid::new("u1")].sidecar.clone().unwrap()
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
                sidecar: remote[&SyncUuid::new("u1")].sidecar.clone().unwrap()
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
                sidecar: remote[&SyncUuid::new("u1")].sidecar.clone().unwrap()
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
                sidecar: remote[&SyncUuid::new("u1")].sidecar.clone().unwrap()
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

    #[test]
    fn remote_only_actions_are_stable_across_hash_order() {
        // Many remote-only notes: regardless of HashMap iteration order,
        // downloads must be emitted in ascending uuid order.
        let uuids = ["c0", "a1", "b2", "e3", "d4"];
        let mut remote = HashMap::new();
        for uuid in uuids {
            let n = note(uuid, uuid, "body", "2026-01-01 10:00:00");
            remote.insert(
                SyncUuid::new(uuid),
                RemoteEntry {
                    sidecar: Some(sidecar_of(&n)),
                    has_md: true,
                },
            );
        }
        let actions = plan_sync(&[], &[], &remote);
        let got: Vec<&str> = actions
            .iter()
            .map(|a| match a {
                SyncAction::Download { sidecar } => sidecar.uuid.as_str(),
                _ => panic!("expected Download, got {a:?}"),
            })
            .collect();
        let mut sorted = got.clone();
        sorted.sort_unstable();
        assert_eq!(got, sorted, "download order must be uuid-sorted");
    }

    #[test]
    fn planner_normalizes_unsorted_duplicate_local_tags() {
        // Local tags arrive unsorted/duplicated from the database; the
        // metadata comparison must not register that as a change.
        let mut local = note("u1", "Zebra", "same", "2026-01-01 10:00:00");
        local.tags = vec!["work".into(), "work ".into(), "alpha".into()];
        let mut remote_note = note("u1", "Apple", "same", "2026-01-01 10:00:00");
        remote_note.tags = vec!["alpha".into(), "work".into()];
        let remote = remote_one(sidecar_of(&remote_note));
        let actions = plan_sync(&[local], &[], &remote);
        // Local title "Zebra" > remote "Apple" -> deterministic download,
        // proving tag ordering/duplication did not masquerade as content
        // change (which would have produced a ConflictCopy).
        assert_eq!(
            actions,
            vec![SyncAction::Download {
                sidecar: remote[&SyncUuid::new("u1")].sidecar.clone().unwrap()
            }]
        );
    }
}
