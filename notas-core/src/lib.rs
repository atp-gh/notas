//! Notas data layer: schema, repository, FTS5 search, export, backup.
//!
//! This crate is deliberately free of any GUI framework: it only talks to
//! SQLite through sqlx and to the filesystem. It is fully unit-tested
//! (`cargo test -p notas-core`).

#![deny(missing_docs)]

pub mod config;
pub mod core;
pub mod crypto;
pub mod db;
pub mod error;
pub mod markdown;
pub mod models;
pub mod notes;
pub mod repo;
pub mod schema;
pub mod search;
pub mod storage;
pub mod sync;
pub mod ui;

pub use core::{AppMsg, DbEvent, DbMsg};
pub use error::{Error, Result};
pub use models::{Note, Notebook, SearchHit, Tag, TagCount};
