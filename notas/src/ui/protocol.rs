//! Platform-neutral UI protocol and navigation state.
//!
//! This module deliberately contains no GTK types. Frontends translate their
//! toolkit events into [`AppMsg`] values, while the application coordinator
//! consumes the same protocol regardless of whether the frontend is GTK,
//! Windows-native, or macOS-native.

pub use notas_core::core::AppMsg;
pub use notas_core::ui::{ViewId, ViewMode};
