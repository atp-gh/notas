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
  buildInputs = [
    gtk4
    libadwaita
    gtksourceview5
    glib
    gdk-pixbuf
    gsettings-desktop-schemas
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
