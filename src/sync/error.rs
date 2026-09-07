//! Typed errors for the sync engine.
//!
//! The sync executor used to report every failure as a plain `String`.
//! [`SyncError`] keeps the same user-facing message quality (each variant's
//! `Display` reads like a sentence) while giving callers a matchable type.
//! It is converted to UI text only at the `DbWorker` boundary.

use crate::core::Error as CoreError;
use crate::sync::crypto::CryptoError;

/// Errors produced by the sync engine (transport, protocol, crypto and the
/// local database steps it drives).
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The backend rejected a request (bad credentials, wrong URL, …).
    #[error("{0}")]
    Transport(String),

    /// A local database step inside the sync failed.
    #[error("sync database step failed: {0}")]
    Database(#[from] sqlx::Error),

    /// A data-core repository step inside the sync failed.
    #[error("sync repository step failed: {0}")]
    Repository(#[from] CoreError),

    /// End-to-end encryption failed (wrong password, bad verifier, …).
    #[error(transparent)]
    Crypto(#[from] CryptoError),

    /// The backend is already encrypted but encryption is disabled, or the
    /// settings are otherwise inconsistent with the remote state.
    #[error("{0}")]
    Configuration(String),
}

impl SyncError {
    /// Build a transport failure from a message.
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport(message.into())
    }

    /// Build a configuration failure from a message.
    pub fn configuration(message: impl Into<String>) -> Self {
        Self::Configuration(message.into())
    }
}

/// Conversion to the legacy string seam. Kept so backend glue can migrate
/// to [`SyncError`] incrementally; new code should match on the variants.
impl From<SyncError> for String {
    fn from(err: SyncError) -> Self {
        err.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_error_displays_its_message() {
        let err = SyncError::transport("WebDAV upload failed (HTTP 401)");
        assert_eq!(err.to_string(), "WebDAV upload failed (HTTP 401)");
    }

    #[test]
    fn sync_error_converts_into_the_string_seam() {
        let err = SyncError::configuration("backend is encrypted");
        let message: String = err.into();
        assert_eq!(message, "backend is encrypted");
    }
}
