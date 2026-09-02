//! Application settings.
//!
//! A small JSON file kept at `$XDG_CONFIG_HOME/notas/settings.json`
//! (falling back to `~/.config/notas/settings.json`), mirroring
//! `notas_core::db::data_dir()` but for config instead of data.
//!
//! Every field has a sensible default: the file may be missing or partial,
//! unknown keys are ignored, and a corrupt file falls back to the defaults
//! (with a logged warning) instead of crashing the app. New settings are
//! added by appending a field with a `Default`-able type, so old files
//! keep loading unchanged.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Color scheme applied app-wide through `adw::StyleManager`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    /// Follow the system / desktop color scheme.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    /// Index into the theme combo row (label order in `settings_window`).
    pub fn index(self) -> u32 {
        match self {
            Self::System => 0,
            Self::Light => 1,
            Self::Dark => 2,
        }
    }

    /// Inverse of [`ThemeMode::index`]; unknown indices map to `System`.
    pub fn from_index(index: u32) -> Self {
        match index {
            1 => Self::Light,
            2 => Self::Dark,
            _ => Self::System,
        }
    }
}

/// Top-level settings, written as one JSON object with namespaced sections.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeSettings,
    pub editor: EditorSettings,
    pub interface: InterfaceSettings,
}

/// Theme section: `"theme": { "mode": "system" }`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    pub mode: ThemeMode,
}

/// Editor section: the user font and the line-number gutter toggle.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// A pango font description string (from `FontDescription::to_str`),
    /// or `None` for the default monospace font.
    pub font_desc: Option<String>,
    pub show_line_numbers: bool,
}

/// Interface section: window chrome toggles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InterfaceSettings {
    pub show_status_bar: bool,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            show_status_bar: true,
        }
    }
}

/// Settings file location: `$XDG_CONFIG_HOME/notas/settings.json`, or
/// `~/.config/notas/settings.json` when the variable is unset or empty.
pub fn settings_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("notas").join("settings.json");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/notas/settings.json")
}

impl Settings {
    /// Read the settings file, falling back to defaults when it is missing,
    /// corrupt, or unreadable (never crashes, never overwrites the file).
    pub fn load() -> Self {
        Self::load_from(settings_path())
    }

    /// Read a specific file; used directly by the tests.
    fn load_from(path: PathBuf) -> Self {
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(settings) => settings,
                Err(err) => {
                    eprintln!(
                        "notas: settings file {} is invalid, using defaults: {err}",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                eprintln!("notas: cannot read settings file {}: {err}", path.display());
                Self::default()
            }
        }
    }

    /// Write the settings atomically (temp file + rename), logging failures.
    pub fn save(&self) {
        let path = settings_path();
        if let Err(err) = self.save_to(&path) {
            eprintln!(
                "notas: failed to save settings to {}: {err}",
                path.display()
            );
        }
    }

    /// Persist to a specific file, replacing it atomically so a crash
    /// mid-write can never leave a truncated settings file behind.
    fn save_to(&self, path: &Path) -> io::Result<()> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        // Unique temp name so concurrent processes never collide.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = dir.join(format!(".settings-{}-{nanos}.tmp", std::process::id()));
        fs::write(&tmp, text)?;
        match fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp path under the system temp dir for one test.
    fn temp_settings_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "notas-settings-test-{name}-{}-{nanos}.json",
            std::process::id()
        ))
    }

    #[test]
    fn defaults_match_designed_values() {
        let s = Settings::default();
        assert_eq!(s.theme.mode, ThemeMode::System);
        assert_eq!(s.editor.font_desc, None);
        assert!(!s.editor.show_line_numbers);
        assert!(s.interface.show_status_bar);
    }

    #[test]
    fn empty_object_gives_defaults() {
        let path = temp_settings_path("empty");
        fs::write(&path, "{}").unwrap();
        let settings = Settings::load_from(path.clone());
        assert_eq!(settings, Settings::default());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_keys_fill_defaults() {
        let path = temp_settings_path("partial");
        fs::write(&path, r#"{"editor": {"show_line_numbers": true}}"#).unwrap();
        let settings = Settings::load_from(path.clone());
        assert_eq!(settings.theme.mode, ThemeMode::System);
        assert_eq!(settings.editor.font_desc, None);
        assert!(settings.editor.show_line_numbers);
        assert!(settings.interface.show_status_bar);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let path = temp_settings_path("unknown");
        fs::write(&path, r#"{"future": {"sync": true}, "editor": {}}"#).unwrap();
        assert_eq!(Settings::load_from(path.clone()), Settings::default());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults_without_overwriting() {
        let path = temp_settings_path("corrupt");
        fs::write(&path, "{ not json !!!").unwrap();
        assert_eq!(Settings::load_from(path.clone()), Settings::default());
        // The corrupt file must not be clobbered: the user may want to fix it.
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json !!!");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let path = temp_settings_path("missing");
        let _ = fs::remove_file(&path);
        assert_eq!(Settings::load_from(path.clone()), Settings::default());
    }

    #[test]
    fn theme_mode_serde_uses_kebab_case() {
        assert_eq!(
            serde_json::from_str::<ThemeMode>("\"system\"").unwrap(),
            ThemeMode::System
        );
        assert_eq!(
            serde_json::from_str::<ThemeMode>("\"light\"").unwrap(),
            ThemeMode::Light
        );
        assert_eq!(
            serde_json::from_str::<ThemeMode>("\"dark\"").unwrap(),
            ThemeMode::Dark
        );
        assert_eq!(serde_json::to_string(&ThemeMode::Dark).unwrap(), "\"dark\"");
    }

    #[test]
    fn theme_mode_indices_roundtrip() {
        for mode in [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark] {
            assert_eq!(ThemeMode::from_index(mode.index()), mode);
        }
    }

    #[test]
    fn save_then_load_roundtrip() {
        let path = temp_settings_path("roundtrip");
        let mut settings = Settings::default();
        settings.theme.mode = ThemeMode::Dark;
        settings.editor.font_desc = Some("Cantarell 14".into());
        settings.editor.show_line_numbers = true;
        settings.interface.show_status_bar = false;

        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(path.clone()), settings);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn save_writes_namespaced_json() {
        let path = temp_settings_path("json");
        Settings::default().save_to(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        // Sections are namespaced and human-readable (pretty-printed).
        assert!(text.contains("\"theme\""), "{text}");
        assert!(text.contains("\"editor\""), "{text}");
        assert!(text.contains("\"interface\""), "{text}");
        assert!(text.contains("\"mode\": \"system\""), "{text}");
        assert!(text.contains("\"show_line_numbers\": false"), "{text}");
        assert!(text.contains("\"show_status_bar\": true"), "{text}");
        let _ = fs::remove_file(&path);
    }
}
