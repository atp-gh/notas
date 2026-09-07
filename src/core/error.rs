//! Error types for the data core.
//!
//! Limited to what the data core itself can produce: database, I/O,
//! serialization, migration, not-found and input-invariant errors. UI and
//! sync-transport failures belong to their own layers.

use crate::core::model::{NoteId, NotebookId, TagId};

/// Errors that can occur in the data core.
///
/// Converted automatically from the underlying failures with `?`; the UI
/// layer renders them via `Display`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database (sqlx/SQLite) failure.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// A filesystem or I/O failure (schema init, export, backup).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A JSON serialization or deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// The stored schema could not be migrated to the current version.
    #[error("schema migration failed: {0}")]
    Migration(String),

    /// A note that was expected to exist does not (anymore).
    #[error("note with id {0} does not exist")]
    NoteNotFound(NoteId),

    /// A notebook that was expected to exist does not (anymore).
    #[error("notebook with id {0} does not exist")]
    NotebookNotFound(NotebookId),

    /// A notebook with this name already exists under the same parent.
    #[error("a notebook named \"{0}\" already exists at this level")]
    NotebookNameExists(String),

    /// A tag that was expected to exist does not (anymore).
    #[error("tag with id {0} does not exist")]
    TagNotFound(TagId),

    /// An input value violates a data-core invariant.
    #[error("{0}")]
    InvalidInput(String),
}

/// Convenience alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;
