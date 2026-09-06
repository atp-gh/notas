//! GTK frontend compatibility exports for persisted application settings.
//!
//! Settings are core data, so their definitions live in `crate::core`; this
//! module keeps the old import path stable while the UI is migrated.

pub use crate::core::config::*;
