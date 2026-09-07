//! Platform-neutral Notas API.
//!
//! The binary frontend lives in `main.rs`; this library target exposes the
//! data core (`core`), the application protocol layer (`application`), the
//! frontend-neutral Markdown projection (`markdown`) and the sync
//! infrastructure (`sync`).
//!
//! Legacy entry points (`domain_notes`, `search`, `storage`) remain as
//! compatibility re-exports until every caller has migrated.

#![deny(missing_docs)]

pub mod application;
pub mod core;
pub mod markdown;
pub mod sync;

#[path = "notes/model.rs"]
pub mod domain_notes;
pub mod search;
pub mod storage;
