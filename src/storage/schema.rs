//! SQLite schema definition.
//!
//! Compatibility shim: the canonical DDL lives in
//! [`crate::core::database`]; keep this module as a thin re-export while
//! callers migrate to the core API.

pub use crate::core::database::SCHEMA_SQL;
