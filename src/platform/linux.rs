use super::PlatformPaths;
use std::path::PathBuf;

pub(super) fn paths() -> PlatformPaths {
    let config_base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    let data_base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    PlatformPaths {
        config_dir: config_base.join("notas"),
        data_dir: data_base.join("notas"),
    }
}
