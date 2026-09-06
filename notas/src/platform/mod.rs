//! Operating-system integration boundary.
//!
//! Business logic receives paths and platform actions through this small
//! abstraction. Toolkit-specific code can be replaced independently on each
//! supported operating system.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Paths used by the application for configuration and data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformPaths {
    /// Directory containing user configuration.
    pub config_dir: PathBuf,
    /// Directory containing mutable application data.
    pub data_dir: PathBuf,
}

/// Returns the platform-specific application paths.
pub fn paths() -> PlatformPaths {
    #[cfg(target_os = "linux")]
    return linux::paths();
    #[cfg(target_os = "macos")]
    return macos::paths();
    #[cfg(target_os = "windows")]
    return windows::paths();
    #[allow(unreachable_code)]
    PlatformPaths {
        config_dir: PathBuf::from(".").join(".config").join("notas"),
        data_dir: PathBuf::from(".")
            .join(".local")
            .join("share")
            .join("notas"),
    }
}
