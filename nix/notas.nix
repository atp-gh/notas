{
  lib,
  rustPlatform,
  pkg-config,
  # GTK4 hook: modern nixpkgs renamed the GTK3 hook to wrapGAppsHook3 and
  # provides wrapGAppsHook4 for GTK4 apps.
  wrapGAppsHook4,
  gtk4,
  libadwaita,
  gtksourceview5,
  adwaita-icon-theme,
  adwaita-icon-theme-legacy,
  hicolor-icon-theme,
  glib,
  gdk-pixbuf,
  gsettings-desktop-schemas,
}:

rustPlatform.buildRustPackage {
  pname = "notas";
  version = "0.1.0";

  src = lib.cleanSource ../.;

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
  ];

  # gtk4-sys / libadwaita-sys / sourceview5-sys probe these via pkg-config at
  # build time and enforce the crate's minimum versions (v4_22 / v1_9), so
  # the nixpkgs snapshot must carry recent-enough GTK. gdk-pixbuf and the
  # gsettings schemas are wrapped in for runtime by wrapGAppsHook4.
  # adwaita-icon-theme is required at runtime: most in-app symbolic icons
  # (document-new, view-dual, open-menu, ...) come from it, and
  # wrapGAppsHook4 only exposes icon themes listed here via XDG_DATA_DIRS.
  # Without it the buttons render blank on NixOS, where there is no global
  # /usr/share/icons to fall back to.
  # adwaita-icon-theme-legacy is required too: nixpkgs' adwaita-icon-theme
  # is NOT FDO-complete (upstream split the FDO/legacy set into a separate
  # AdwaitaLegacy theme and nixpkgs patches out the inheritance), so
  # category/legacy names only resolve when the legacy theme is also on
  # XDG_DATA_DIRS. hicolor-icon-theme is the ultimate fallback all themes
  # inherit, so it must be present as well.
  # NOTE: this only helps when the user's icon theme falls back to Adwaita
  # (e.g. gtk4 iconTheme = Adwaita). Themes like Papirus-Dark inherit
  # breeze-dark/hicolor — never Adwaita — so icons missing in Papirus itself
  # stay blank regardless of what the wrapper ships; see src/ui/icons.rs.
  buildInputs = [
    gtk4
    libadwaita
    gtksourceview5
    adwaita-icon-theme
    adwaita-icon-theme-legacy
    glib
    gdk-pixbuf
    gsettings-desktop-schemas
    hicolor-icon-theme
  ];

  # All sqlx queries are runtime (no query! macros), so no DATABASE_URL or
  # offline mode is needed at build time. GUI probes are #[ignore]d, so the
  # test suite runs headless.
  doCheck = true;

  meta = {
    description = "A fast, local-first note-taking app for Linux, written in Rust";
    homepage = "https://github.com/atp-gh/notas";
    license = lib.licenses.mit;
    mainProgram = "notas";
    platforms = lib.platforms.linux;
  };
}
