use super::PlatformPaths;
use std::path::PathBuf;

pub(super) fn paths() -> PlatformPaths {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let app_support = home.join("Library/Application Support/Notas");
    PlatformPaths {
        config_dir: app_support.join("config"),
        data_dir: app_support.join("data"),
    }
}
