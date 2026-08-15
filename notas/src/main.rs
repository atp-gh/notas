mod app;
mod db_worker;
mod editor;
#[macro_use]
mod tr;

use std::process::ExitCode;

use relm4::RelmApp;

use crate::app::App;

fn main() -> ExitCode {
    let relm_app = RelmApp::new("io.github.notas.Notas");

    // Connect to SQLite before the UI starts. The runtime is intentionally
    // leaked: sqlx may keep background tasks alive on it for the pool.
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let pool = rt
        .block_on(notas_core::db::connect(notas_core::db::db_path()))
        .unwrap_or_else(|e| {
            eprintln!("failed to open database: {e:#}");
            std::process::exit(1);
        });
    std::mem::forget(rt);

    relm_app.run::<App>(pool);
    ExitCode::SUCCESS
}
