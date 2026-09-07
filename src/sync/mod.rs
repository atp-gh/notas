//! Sync planning and execution.
//!
//! The pure planner lives in [`crate::core::sync`]; this module tree keeps
//! the infrastructure side: typed errors ([`error`]), encryption
//! primitives ([`crypto`]) and the backend executors ([`executor`]).

pub mod crypto;
pub mod error;
pub mod executor;

pub use crate::core::sync::{
    LocalNote, RemoteEntry, Sidecar, SyncAction, SyncStats, content_hash, plan_sync,
};
pub use error::SyncError;
