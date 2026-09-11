//! Central registry of the symbolic icon names used across the UI.
//!
//! All names are kept here (instead of scattered string literals) so a
//! missing-icon report only requires checking one place, and a rename only
//! touches one line per icon.
//!
//! # Why these names
//!
//! The `*-symbolic` set below is GNOME stock: every name resolves in the
//! Adwaita theme that libadwaita apps target. On NixOS there is no global
//! `/usr/share/icons`, so the wrapper must ship the themes explicitly (see
//! `nix/notas.nix`: `adwaita-icon-theme` + `adwaita-icon-theme-legacy` +
//! `hicolor-icon-theme` on `XDG_DATA_DIRS`).
//!
//! One caveat the wrapper cannot fix: GTK only falls back through the
//! *current theme's* `Inherits` chain. `Papirus-Dark` inherits
//! `breeze-dark,hicolor` — never Adwaita — so an icon that Papirus itself
//! lacks stays blank even when Adwaita (with the icon) is installed. If an
//! icon goes missing under a third-party theme, prefer a name that exists
//! in that theme too, rather than adding more themes to the wrapper.
//!
//! Concretely: `applications-graphics-symbolic` is a legacy fullcolor
//! category name (upstream moved `applications-graphics` to the legacy set
//! years ago). It is missing in Papirus-Dark/breeze chains, so the
//! Appearance page uses the modern HIG name
//! `preferences-desktop-appearance-symbolic` instead, which ships in both
//! Adwaita and Papirus (added upstream in Papirus 20250501).

/// New-note button in the note-list header.
pub const NEW_NOTE: &str = "document-new-symbolic";
/// Save button in the editor status bar.
pub const SAVE: &str = "document-save-symbolic";
/// Move-to-trash button in the header bar.
pub const TRASH: &str = "user-trash-symbolic";
/// "Editor only" toggle in the header-bar mode switch.
pub const MODE_SOURCE: &str = "document-edit-symbolic";
/// "Split: editor + live preview" toggle in the header-bar mode switch.
pub const MODE_SPLIT: &str = "view-dual-symbolic";
/// "Preview only" toggle in the header-bar mode switch.
pub const MODE_PREVIEW: &str = "view-reveal-symbolic";
/// Hamburger menu in the header bar.
pub const MENU: &str = "open-menu-symbolic";
/// New-notebook button in the sidebar.
pub const NEW_NOTEBOOK: &str = "folder-new-symbolic";
/// Icon of the Appearance page in the settings dialog.
///
/// Modern HIG name on purpose: the legacy `applications-graphics-symbolic`
/// does not resolve under Papirus-Dark/breeze chains (see module docs).
pub const SETTINGS_APPEARANCE: &str = "preferences-desktop-appearance-symbolic";
/// Icon of the Sync page in the settings dialog.
pub const SETTINGS_SYNC: &str = "folder-remote-symbolic";
