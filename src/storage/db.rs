//! Database connection setup.
//!
//! Compatibility shim: the implementation lives in
//! [`crate::core::database`]; keep this module as a thin re-export while
//! callers migrate to the core API.

pub use crate::core::database::connect;
