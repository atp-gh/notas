//! Central registry of the symbolic icon names used across the UI.
//!
//! All names are kept here (instead of scattered string literals) so a
//! missing-icon report only requires checking one place, and a rename only
//! touches one line per icon.
//!
//! # No-legacy policy
//!
//! Every name below is in the **Adwaita ∩ Papirus intersection**: it
//! resolves both under Adwaita (shipped by the Nix wrapper, see
//! `nix/notas.nix`) and under Papirus / Papirus-Dark (whose `Inherits`
//! chain is `breeze-dark,hicolor` and therefore never falls back to
//! Adwaita). Because of that, the wrapper deliberately does **not** ship
//! `adwaita-icon-theme-legacy` — no name here needs it.
//!
//! Verified against nixpkgs' `adwaita-icon-theme-50.0`
//! (`share/icons/Adwaita/symbolic/...`) and upstream Papirus master
//! (`Papirus/16x16/symbolic/...`, which `Papirus-Dark` symlinks to):
//!
//! - `document-new`, `document-save`, `document-edit`, `list-add`,
//!   `view-dual`, `view-reveal`, `open-menu` → both themes (`actions`).
//! - `user-trash`, `folder-remote` → both themes (`places`).
//! - `preferences-system` → both themes (`categories`).
//!
//! Two names that look obvious but are **not** portable, so they are not
//! used: `folder-new-symbolic` (Adwaita-only, Papirus has no such file, so
//! the new-notebook button uses `list-add-symbolic`) and
//! `preferences-desktop-appearance-symbolic` (Adwaita-only, Papirus has no
//! such file, so the Appearance page uses `preferences-system-symbolic`).
//!
//! # Adding a new icon
//!
//! Pick a name that exists in **both** themes (check the two locations
//! above, e.g. with `gtk4-icon-browser` under each theme) and add it here
//! with a doc comment. Never add a name that only resolves via the legacy
//! theme.

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
///
/// `folder-new-symbolic` is Adwaita-only (Papirus ships no such file), so
/// the portable "add" glyph is used instead; the tooltip still says
/// "New notebook".
pub const NEW_NOTEBOOK: &str = "list-add-symbolic";
/// Icon of the Appearance page in the settings dialog.
///
/// `preferences-desktop-appearance-symbolic` is Adwaita-only (Papirus ships
/// no such file), so the portable settings gear is used instead.
pub const SETTINGS_APPEARANCE: &str = "preferences-system-symbolic";
/// Icon of the Sync page in the settings dialog.
pub const SETTINGS_SYNC: &str = "folder-remote-symbolic";
