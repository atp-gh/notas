//! Regression test: core UI flows must not emit Gtk criticals.
//!
//! Covers the reported `gtk_css_node_insert_after: assertion
//! 'previous_sibling == NULL || previous_sibling->parent == parent' failed`,
//! seen at startup and when clicking a notebook.
//!
//! Root cause found via a Gtk log trap: `gtk::Popover::set_parent` onto the
//! notebook `TreeView` trips `gtk_widget_reposition_after` even on an empty,
//! unrooted popover (a GTK-side fragility around the TreeView's CSS nodes).
//! The notebook context menu is therefore parented to the surrounding
//! `ScrolledWindow` instead — see `build_tree_pane`. If this test goes red,
//! re-introduce the trap's stack capture (git history) to find which widget
//! operation started tripping again.
//!
//! Headless-safe by design (no `present()`): the critical fires on unrooted
//! widget operations, so this runs under nix `doCheck` without a display.

use std::sync::Mutex;

use libadwaita as adw;
use relm4::ComponentController;

use notas::core::database;
use notas::core::repository::Repository;

use crate::app::{App, AppInit};
use crate::ui::AppMsg;

/// Stacks of the reported critical, capped so a spamming path can't OOM the
/// suite. Printed only when the test fails.
static CSS_STACKS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Route Gtk criticals through an in-process trap. The reported critical
/// records a stack; everything else is forwarded to the default handler so
/// no diagnostic is swallowed.
fn install_css_trap() {
    glib::log_set_handler(
        Some("Gtk"),
        glib::LogLevels::LEVEL_CRITICAL,
        false,
        false,
        |domain, level, message| {
            if message.contains("gtk_css_node_insert_after") {
                let mut stacks = CSS_STACKS.lock().expect("stack lock");
                if stacks.len() < 3 {
                    stacks.push(format!("{:?}", std::backtrace::Backtrace::force_capture()));
                }
            } else {
                glib::log_default_handler(domain, level, Some(message));
            }
        },
    );
}

/// Run main-loop iterations for `ms` milliseconds. A periodic ticker source
/// is registered first so `iteration(true)` can never block on an empty
/// event queue.
fn pump_ms(ms: u64) {
    let ctx = glib::MainContext::default();
    let _ticker = glib::timeout_add_local(std::time::Duration::from_millis(5), || {
        glib::ControlFlow::Continue
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        ctx.iteration(true);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn css_hits() -> usize {
    CSS_STACKS.lock().expect("stack lock").len()
}

/// Startup, notebook selection (flat + nested) and note opening (including a
/// Mermaid diagram, which embeds widgets at text anchors) emit no
/// `gtk_css_node_insert_after` criticals.
#[test]
fn ui_flows_emit_no_gtk_criticals() {
    gtk::init().expect("gtk init");
    adw::init().expect("adw init");
    install_css_trap();

    // The tokio runtime must outlive the pool (sqlx keeps background tasks
    // on it), so it is declared first and dropped last.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let dir = tempfile::tempdir().expect("tempdir");
    let pool = rt
        .block_on(database::connect(dir.path().join("repro.db")))
        .expect("connect");
    let repo = Repository::new(pool.clone());
    let work = rt
        .block_on(repo.create_notebook(None, "Work"))
        .expect("seed parent notebook");
    let sub = rt
        .block_on(repo.create_notebook(Some(work.id), "Sub"))
        .expect("seed child notebook");
    for title in ["alpha", "beta", "gamma"] {
        let note = rt
            .block_on(repo.create_note(Some(work.id), title))
            .expect("seed note");
        rt.block_on(repo.update_note(note.id, title, "# Hello\n\nbody text\n"))
            .expect("seed content");
    }
    let diagram = rt
        .block_on(repo.create_note(Some(work.id), "diagram"))
        .expect("seed diagram note");
    rt.block_on(repo.update_note(
        diagram.id,
        "diagram",
        "# Flow\n\n```mermaid\ngraph TD\nA --> B\n```\n",
    ))
    .expect("seed diagram content");

    let settings = crate::application::config::Settings::default();
    let controller = relm4::ComponentBuilder::<App>::default().launch(AppInit { pool, settings });

    // Let the startup loads (notebooks/tags/notes) land and render.
    pump_ms(2500);
    let startup_hits = css_hits();

    // The exact message the TreeView `row_activated` handler sends on click.
    controller.emit(AppMsg::SelectNotebook(work.id));
    pump_ms(2500);
    let click_hits = css_hits() - startup_hits;

    // Same for a nested notebook (different tree path).
    controller.emit(AppMsg::SelectNotebook(sub.id));
    pump_ms(2500);
    let nested_hits = css_hits() - startup_hits - click_hits;

    // Open the diagram note: loads content into the editor and runs the
    // async preview render with Mermaid anchor embedding.
    controller.emit(AppMsg::SelectNote(diagram.id));
    pump_ms(4000);
    let note_hits = css_hits() - startup_hits - click_hits - nested_hits;

    if css_hits() > 0 {
        for (i, stack) in CSS_STACKS.lock().expect("stack lock").iter().enumerate() {
            eprintln!("gtk critical stack #{i}:\n{stack}");
        }
    }
    assert_eq!(startup_hits, 0, "startup emitted gtk_css_node_insert_after");
    assert_eq!(
        click_hits, 0,
        "SelectNotebook emitted gtk_css_node_insert_after"
    );
    assert_eq!(
        nested_hits, 0,
        "SelectNotebook (nested) emitted gtk_css_node_insert_after"
    );
    assert_eq!(
        note_hits, 0,
        "SelectNote (mermaid) emitted gtk_css_node_insert_after"
    );
}
