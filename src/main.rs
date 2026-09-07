mod app;
mod editor;
mod notes;
mod platform;
mod ui;
#[macro_use]
mod tr;

// Reuse the library target's single core/sync module instances. Keeping the
// binary as a thin frontend avoids compiling two distinct copies of core
// types (which can otherwise make integration between targets surprising).
#[allow(dead_code)]
pub use notas::domain_notes;
pub use notas::{application, core, markdown, search, storage, sync};

use std::process::ExitCode;

use relm4::RelmApp;

use crate::app::{App, AppInit};
use crate::application::config::Settings;

fn main() -> ExitCode {
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
    let pool = match rt.block_on(crate::storage::db::connect(
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
    let relm_app = RelmApp::new("io.github.notas.Notas");
    relm_app.run::<App>(AppInit { pool, settings });
    ExitCode::SUCCESS
}
