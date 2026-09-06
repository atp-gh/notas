//! Platform-neutral Notas core API.
//!
//! The binary frontend lives in `main.rs`; this library target exposes the
//! same `core` tree so integration tests and future frontends can reuse it.

pub mod core;

pub use core::config;
pub use core::crypto;
pub use core::db;
pub use core::models;
pub use core::repo;
pub use core::schema;
pub use core::sync;
