mod app;
mod editor;
mod notes;
mod platform;
mod ui;
#[macro_use]
mod tr;

#[cfg(test)]
mod gtk_regressions;

// Reuse the library target's single core/sync module instances. Keeping the
// binary as a thin frontend avoids compiling two distinct copies of core
// types (which can otherwise make integration between targets surprising).
// The modules below address the library as `notas::…`; this re-export only
// pulls them into the binary as one shared instance.
pub use notas::{application, core, markdown, sync};

use std::process::ExitCode;

use relm4::RelmApp;

use crate::app::{App, AppInit};
use crate::application::config::Settings;

fn main() -> ExitCode {
    // The library layers emit `tracing` events and never install a
    // subscriber; the binary owns that choice. Default to `warn` so the
    // skip-and-continue sync branches stay visible on stderr, honoring
    // `RUST_LOG` when set (e.g. `RUST_LOG=notas=debug`).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let platform_paths = platform::paths();

    // Connect to SQLite before the UI starts. The dedicated tokio runtime
    // must outlive the app: the DB worker and the sqlx pool keep running
    // tasks on it for as long as the window is open. Merely keeping `rt` in
    // scope is enough here, because `RelmApp::run` blocks until the app
    // exits (no need to leak it with `mem::forget`).
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to create tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let pool = match rt.block_on(notas::core::database::connect(
        platform_paths.data_dir.join("notas.db"),
    )) {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("failed to open database: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Resolve platform paths once during startup; platform-specific services
    // can be injected into core without exposing GTK or OS environment APIs.
    let settings = Settings::load_from(platform_paths.config_dir.join("settings.json"));
    let resources_dir = platform_paths.data_dir.join("resources");
    let relm_app = RelmApp::new("io.github.notas.Notas");
    relm_app.run::<App>(AppInit {
        pool,
        settings,
        resources_dir,
    });
    ExitCode::SUCCESS
}
