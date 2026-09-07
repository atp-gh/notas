//! Platform-neutral Notas API.
//!
//! The binary frontend lives in `main.rs`; this library target exposes the
//! data core (`core`) plus its infrastructure siblings (`sync`) so
//! integration tests and future frontends can reuse them.
//!
//! Legacy entry points (`domain_notes`, `search`, `storage`) remain as
//! compatibility re-exports until every caller has migrated.

#![deny(missing_docs)]

pub mod core;
pub mod sync;

#[path = "notes/model.rs"]
pub mod domain_notes;
pub mod search;
pub mod storage;
