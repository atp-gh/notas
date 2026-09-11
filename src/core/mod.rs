//! GUI/platform-free data core.
//!
//! This module intentionally contains no GUI, application-protocol or
//! markdown-rendering types. Storage, search, notes and synchronization
//! are sibling services; the application layer (see [`crate::application`])
//! translates frontend events into commands and renders service results.

#![deny(missing_docs)]

pub mod database;
pub mod error;
pub mod model;
pub mod repository;
pub mod resources;
pub mod search;
pub mod sync;
