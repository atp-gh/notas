use super::PlatformPaths;
use std::path::PathBuf;

pub(super) fn paths() -> PlatformPaths {
    let config = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let data = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| config.clone());
    PlatformPaths {
        config_dir: config.join("Notas"),
        data_dir: data.join("Notas"),
    }
}
