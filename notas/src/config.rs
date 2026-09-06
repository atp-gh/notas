//! Application settings.
//!
//! A small JSON file kept at `$XDG_CONFIG_HOME/notas/settings.json`
//! (falling back to `~/.config/notas/settings.json`), mirroring
//! `notas_core::db::data_dir()` but for config instead of data.
//!
//! Every field has a sensible default: the file may be missing or partial,
//! unknown keys are ignored, and a corrupt file falls back to the defaults
//! (with a logged warning) instead of crashing the app. Unknown keys stay
//! inert, so a file written by a newer app version keeps loading. The one
//! exception is the sync section's own layout change (see
//! [`SyncSettings`]): its old flat S3 fields are no longer read.
//!
//! Settings are stored in plaintext (sync credentials included), matching
//! apps like Joplin; the README explains how to scope each credential.

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
    pub sync: SyncSettings,
}

/// Theme section: `"theme": { "mode": "system" }`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    pub mode: ThemeMode,
}

/// Editor section: the line-number gutter toggle.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
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

/// Which backend the notes sync to. The settings dialog's "Sync type"
/// combo lists these; both have working engines today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncType {
    /// S3-compatible object storage (AWS, Cloudflare R2, Backblaze B2,
    /// MinIO, …).
    #[default]
    S3,
    /// WebDAV (Nextcloud, ownCloud, …). Serde needs the explicit name:
    /// `rename_all = "kebab-case"` would split the acronym into
    /// `web-d-a-v`.
    #[serde(rename = "webdav")]
    WebDAV,
}

impl SyncType {
    /// Index into the sync-type combo row (label order in `settings_window`).
    pub fn index(self) -> u32 {
        match self {
            Self::S3 => 0,
            Self::WebDAV => 1,
        }
    }

    /// Inverse of [`SyncType::index`]; unknown indices map to `S3`.
    pub fn from_index(index: u32) -> Self {
        match index {
            1 => Self::WebDAV,
            _ => Self::S3,
        }
    }
}

/// S3-compatible object storage target.
///
/// Credentials are secrets — the README recommends a dedicated,
/// bucket-scoped credential that can be revoked independently of the main
/// account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct S3SyncSettings {
    /// Custom endpoint URL (`https://<account>.r2.cloudflarestorage.com`
    /// for R2, MinIO's address, …). Empty means AWS S3.
    pub endpoint: String,
    /// Signing region; `us-east-1` for AWS, `auto` for Cloudflare R2.
    pub region: String,
    /// Bucket name.
    pub bucket: String,
    /// Key prefix inside the bucket (`notas/` by default).
    pub prefix: String,
    /// S3 access key id.
    pub access_key_id: String,
    /// S3 secret access key.
    pub secret_access_key: String,
}

impl Default for S3SyncSettings {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            region: "us-east-1".into(),
            bucket: String::new(),
            prefix: "notas/".into(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
        }
    }
}

impl S3SyncSettings {
    /// Whether a sync can even be attempted: a bucket and both credentials
    /// must be present.
    pub fn is_configured(&self) -> bool {
        !self.bucket.trim().is_empty()
            && !self.access_key_id.trim().is_empty()
            && !self.secret_access_key.trim().is_empty()
    }
}

/// End-to-end encryption for synced notes.
///
/// When enabled, every object uploaded to the backend (markdown bodies and
/// sidecars alike) is sealed with XChaCha20-Poly1305 under a key derived
/// from [`EncryptionSettings::password`] via Argon2id — the backend only
/// ever sees ciphertext. The password is stored in plaintext in the
/// settings file like the sync credentials (Joplin-style): it protects
/// against backend/server compromise, **not** against theft of this
/// machine. It must be re-entered on every device that syncs.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EncryptionSettings {
    /// Whether uploaded objects are encrypted.
    pub enabled: bool,
    /// The encryption password (≥ 8 characters when enabled). Shared
    /// across both sync backends; same value needed on every device.
    pub password: String,
}

/// WebDAV target (Nextcloud, ownCloud, …).
///
/// The URL points at a DAV collection the user can write to (for
/// Nextcloud, `…/remote.php/dav/files/<user>`); an optional `directory`
/// (default `notas`) is created below it and holds the notes. Credentials
/// are stored in plaintext in the settings file like the S3 keys — the
/// README recommends a Nextcloud *app password*.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebDavSyncSettings {
    /// DAV collection URL (https), or http when `insecure_tls` is set.
    pub url: String,
    /// Username for Basic auth.
    pub username: String,
    /// Password for Basic auth.
    pub password: String,
    /// Subfolder under the URL holding the notes (`notas` by default;
    /// empty syncs straight into the URL).
    pub directory: String,
    /// Accept self-signed / invalid certificates and plain `http://`.
    /// Off by default; turning it on sends credentials in plaintext over
    /// plain http and skips certificate checks.
    pub insecure_tls: bool,
}

impl Default for WebDavSyncSettings {
    fn default() -> Self {
        Self {
            url: String::new(),
            username: String::new(),
            password: String::new(),
            directory: "notas".into(),
            insecure_tls: false,
        }
    }
}

impl WebDavSyncSettings {
    /// Whether a sync can even be attempted: a URL and both credentials
    /// must be present.
    pub fn is_configured(&self) -> bool {
        !self.url.trim().is_empty()
            && !self.username.trim().is_empty()
            && !self.password.trim().is_empty()
    }
}

/// Sync section: sync backend plus per-backend target sections.
///
/// Both backends keep their own section so switching `kind` in the
/// settings dialog never discards the other backend's configuration. The
/// fields live in the same plaintext settings file as everything else
/// (see `Settings::load`).
///
/// Note: the settings file layout changed once (v0.1): the old flat S3
/// fields (`endpoint`, `bucket`, …) at the top of the sync section are no
/// longer read — re-enter them under the `s3` section after upgrading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncSettings {
    /// Sync backend whose section is used by `run_sync`.
    pub kind: SyncType,
    /// S3 target (kept even while `kind` is WebDAV).
    pub s3: S3SyncSettings,
    /// WebDAV target (kept even while `kind` is S3).
    pub webdav: WebDavSyncSettings,
    /// End-to-end encryption, shared by both backends.
    pub encryption: EncryptionSettings,
    /// Time of the last successful sync (`YYYY-MM-DD HH:MM:SS`, device
    /// local time), or empty if never.
    pub last_synced_at: String,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            kind: SyncType::S3,
            s3: S3SyncSettings::default(),
            webdav: WebDavSyncSettings::default(),
            encryption: EncryptionSettings::default(),
            last_synced_at: String::new(),
        }
    }
}

impl SyncSettings {
    /// Whether a sync can even be attempted for the selected backend: the
    /// backend's own `is_configured` must pass.
    pub fn is_configured(&self) -> bool {
        match self.kind {
            SyncType::S3 => self.s3.is_configured(),
            SyncType::WebDAV => self.webdav.is_configured(),
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
        assert!(!s.editor.show_line_numbers);
        assert!(s.interface.show_status_bar);
        // Sync defaults: S3 backend with AWS defaults and a `notas/`
        // prefix; the WebDAV section defaults to a `notas` subfolder.
        // Neither backend is configured yet.
        assert_eq!(s.sync.kind, SyncType::S3);
        assert_eq!(s.sync.s3.region, "us-east-1");
        assert_eq!(s.sync.s3.prefix, "notas/");
        assert!(s.sync.s3.endpoint.is_empty());
        assert_eq!(s.sync.webdav.directory, "notas");
        assert!(s.sync.webdav.url.is_empty());
        assert!(!s.sync.webdav.insecure_tls);
        // Encryption: off by default, no password.
        assert!(!s.sync.encryption.enabled);
        assert!(s.sync.encryption.password.is_empty());
        assert!(!s.sync.is_configured());
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
        settings.editor.show_line_numbers = true;
        settings.interface.show_status_bar = false;
        settings.sync.kind = SyncType::WebDAV;
        settings.sync.s3.bucket = "my-notes".into();
        settings.sync.s3.endpoint = "https://s3.example.com".into();
        settings.sync.s3.access_key_id = "AK".into();
        settings.sync.s3.secret_access_key = "SK".into();
        settings.sync.webdav.url = "https://nc.example/remote.php/dav/files/alice".into();
        settings.sync.webdav.username = "alice".into();
        settings.sync.webdav.password = "app-pw".into();
        settings.sync.webdav.directory = "My Notes".into();
        settings.sync.webdav.insecure_tls = true;
        settings.sync.encryption.enabled = true;
        settings.sync.encryption.password = "correct horse battery staple".into();
        settings.sync.last_synced_at = "2026-01-01 00:00:00".into();

        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(path.clone()), settings);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn save_writes_namespaced_json() {
        let path = temp_settings_path("json");
        Settings::default().save_to(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        // Sections are namespaced and human-readable (pretty-printed);
        // both sync backends keep their own section.
        assert!(text.contains("\"theme\""), "{text}");
        assert!(text.contains("\"editor\""), "{text}");
        assert!(text.contains("\"interface\""), "{text}");
        assert!(text.contains("\"sync\""), "{text}");
        assert!(text.contains("\"mode\": \"system\""), "{text}");
        assert!(text.contains("\"show_line_numbers\": false"), "{text}");
        assert!(text.contains("\"show_status_bar\": true"), "{text}");
        assert!(text.contains("\"kind\": \"s3\""), "{text}");
        assert!(text.contains("\"s3\""), "{text}");
        assert!(text.contains("\"prefix\": \"notas/\""), "{text}");
        assert!(text.contains("\"webdav\""), "{text}");
        assert!(text.contains("\"url\": \"\""), "{text}");
        assert!(text.contains("\"directory\": \"notas\""), "{text}");
        assert!(text.contains("\"encryption\""), "{text}");
        assert!(text.contains("\"enabled\": false"), "{text}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sync_type_serde_uses_kebab_case() {
        for (kind, name) in [(SyncType::S3, "s3"), (SyncType::WebDAV, "webdav")] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), format!("\"{name}\""));
            assert_eq!(
                serde_json::from_str::<SyncType>(&format!("\"{name}\"")).unwrap(),
                kind
            );
        }
    }

    #[test]
    fn sync_type_indices_roundtrip() {
        for kind in [SyncType::S3, SyncType::WebDAV] {
            assert_eq!(SyncType::from_index(kind.index()), kind);
        }
    }

    #[test]
    fn is_configured_depends_on_the_selected_backend() {
        let mut settings = Settings::default();
        assert!(!settings.sync.is_configured());

        // Filling the S3 section configures the default S3 kind.
        settings.sync.s3.bucket = "b".into();
        settings.sync.s3.access_key_id = "AK".into();
        settings.sync.s3.secret_access_key = "SK".into();
        assert!(settings.sync.is_configured());

        // Switching to WebDAV checks the WebDAV section instead, which is
        // still empty.
        settings.sync.kind = SyncType::WebDAV;
        assert!(!settings.sync.is_configured());

        // WebDAV needs url + username + password.
        settings.sync.webdav.url = "https://nc.example".into();
        assert!(!settings.sync.is_configured());
        settings.sync.webdav.username = "alice".into();
        settings.sync.webdav.password = "pw".into();
        assert!(settings.sync.is_configured());

        // A whitespace-only password does not count.
        settings.sync.webdav.password = " ".into();
        assert!(!settings.sync.is_configured());
    }
}
