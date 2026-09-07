//! Application-wide GTK theme integration.

use libadwaita as adw;

use crate::application::config::ThemeMode;

/// Apply the persisted color scheme to libadwaita.
pub(crate) fn apply(mode: ThemeMode) {
    let scheme = match mode {
        ThemeMode::System => adw::ColorScheme::Default,
        ThemeMode::Light => adw::ColorScheme::ForceLight,
        ThemeMode::Dark => adw::ColorScheme::ForceDark,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}
