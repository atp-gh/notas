mod app;
mod db_worker;
mod editor;
#[macro_use]
mod tr;

use std::process::ExitCode;

use relm4::RelmApp;

use crate::app::App;

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
    let pool = match rt.block_on(notas_core::db::connect(notas_core::db::db_path())) {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("failed to open database: {e}");
            return ExitCode::FAILURE;
        }
    };

    let relm_app = RelmApp::new("io.github.notas.Notas");
    relm_app.run::<App>(pool);
    ExitCode::SUCCESS
}
