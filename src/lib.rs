//! Platform-neutral Notas API.
//!
//! The binary frontend lives in `main.rs`; this library target exposes the
//! data core (`core`), the application protocol layer (`application`), the
//! frontend-neutral Markdown projection (`markdown`) and the sync
//! infrastructure (`sync`).

#![deny(missing_docs)]

pub mod application;
pub mod core;
pub mod markdown;
pub mod sync;
