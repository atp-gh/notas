mod app;
pub mod core;
mod editor;
mod notes;
mod platform;
pub mod sync;
mod ui;
#[macro_use]
mod tr;

use std::process::ExitCode;

use relm4::RelmApp;

use crate::app::{App, AppInit};
use crate::core::config::Settings;

fn main() -> ExitCode {
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
    let pool = match rt.block_on(crate::core::storage::db::connect(
        crate::core::storage::db::db_path(),
    )) {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("failed to open database: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Resolve platform paths once during startup; platform-specific services
    // can be injected into core without exposing GTK or OS environment APIs.
    let platform_paths = platform::paths();
    let settings = Settings::load_from(platform_paths.config_dir.join("settings.json"));
    let relm_app = RelmApp::new("io.github.notas.Notas");
    relm_app.run::<App>(AppInit { pool, settings });
    ExitCode::SUCCESS
}
