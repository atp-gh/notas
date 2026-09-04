//! Notas data layer: schema, repository, FTS5 search, export, backup.
//!
//! This crate is deliberately free of any GUI framework: it only talks to
//! SQLite through sqlx and to the filesystem. It is fully unit-tested
//! (`cargo test -p notas-core`).

#![deny(missing_docs)]

pub mod db;
pub mod error;
pub mod models;
pub mod repo;
pub mod schema;
pub mod sync;

pub use error::{Error, Result};
pub use models::{Note, Notebook, SearchHit, Tag, TagCount};
