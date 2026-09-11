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
  # adwaita-icon-theme is required at runtime: the in-app symbolic icons
  # resolve from it when the user's theme falls back to Adwaita, and
  # wrapGAppsHook4 only exposes icon themes listed here via XDG_DATA_DIRS.
  # Without it the buttons render blank on NixOS, where there is no global
  # /usr/share/icons to fall back to. hicolor-icon-theme is the ultimate
  # fallback every theme inherits, so it must be present as well.
  # Deliberately NO adwaita-icon-theme-legacy: the app only uses icon names
  # from the Adwaita ∩ Papirus intersection (see src/ui/icons.rs), so the
  # legacy theme is never needed — under either theme family every button
  # resolves without it.
  buildInputs = [
    gtk4
    libadwaita
    gtksourceview5
    adwaita-icon-theme
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
