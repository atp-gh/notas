//! SQLite and filesystem persistence boundary.
//!
//! Compatibility shim: the database lifecycle now lives in
//! [`crate::core::database`]; this module re-exports it for existing
//! callers during the migration.

pub mod db;
pub mod repo;
pub mod schema;

pub use crate::core::database::SCHEMA_SQL;
