//! Platform-neutral UI protocol and navigation state.
//!
//! This module deliberately contains no GTK types. Frontends translate their
//! toolkit events into [`AppMsg`] values, while the application coordinator
//! consumes the same protocol regardless of whether the frontend is GTK,
//! Windows-native, or macOS-native.

pub use crate::application::AppMsg;
pub use crate::application::ViewMode;
