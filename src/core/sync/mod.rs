//! Pure sync planning: conflict, deletion and tombstone decisions.
//!
//! This module is GUI-, database-, network- and crypto-free: it only
//! decides *what* needs to happen given a local index and a remote index;
//! the transport executor (see [`crate::sync::executor`]) turns the plan
//! into actual store and database operations.
//!
//! ## Model
//!
//! Every note is identified across devices by a stable UUID; the local
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

pub mod model;
pub mod planner;

pub use model::{LocalNote, RemoteEntry, Sidecar, SyncAction, SyncStats};
pub use planner::{content_hash, plan_sync};
