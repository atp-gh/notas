//! Error types for the platform-neutral core data layer.

/// Errors that can occur in the data layer.
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

    /// A note that was expected to exist does not (anymore).
    #[error("note with id {0} does not exist")]
    NoteNotFound(i64),

    /// An encryption failure from [`crate::core::sync::crypto`].
    #[error(transparent)]
    Crypto(#[from] crate::core::sync::crypto::CryptoError),
}

/// Convenience alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;
