//! Window layout: the initial three-pane divider split and its probes.
//!
//! Pane geometry is pure `GtkPaned` wiring — it owns no app state and is
//! independent of the component that builds the panes — so it lives in its
//! own module together with the manual display probes that keep it honest
//! (those need a real screen and are therefore `#[ignore]`d).

use gtk::prelude::*;

/// Establish the initial 1:1:2 split (sidebar | note list | editor) once the
/// panes have their first real allocation, then leave every later resize to
/// GtkPaned's native proportional behaviour.
///
/// GtkPaned already scales the divider position with the allocation when
/// both children are resizeable (the default: `position = new_width *
/// (old_position / old_width)` in `gtk_paned_calc_position`), so the app
/// only needs to set the split once and never touch it again. But that one
/// set is required: with no position ever set, GtkPaned's first layout
/// places the divider from the children's **minimum** sizes, and a nested
/// paned's minimum is ~0 (its children default to shrinkable), so the
/// divider lands at the far left and GtkPaned overflows both children past
/// the window edges instead of clipping them. Setting the position once
/// switches it into the proportional mode above.
///
/// The split must be derived from the panes' **own allocated widths**, not
/// from the surface width: on Wayland the surface reports a new size at
/// configure time, before the widgets reallocate, so a position computed
/// from it would be interpreted against stale geometry on the next layout
/// and drift out of the window.
///
/// An idle callback is used instead of a property notification because
/// GtkWidget has no `width` property in GTK4 to hook; it reschedules until
/// the panes report real widths. The two halves must be applied in
/// sequence: the outer split is what gives the inner paned its real width,
/// so the callback parks on the outer split first and only splits the inner
/// paned once a later layout pass has allocated it (a fresh `Continue` lets
/// that re-layout happen between idle rounds).
pub(super) fn wire_initial_split(outer_pane: &gtk::Paned, inner_pane: &gtk::Paned) {
    let outer_pane = outer_pane.clone();
    let inner_pane = inner_pane.clone();
    let mut outer_split = false;
    glib::idle_add_local(move || {
        if !outer_split {
            let outer_w = outer_pane.width();
            if outer_w > 100 {
                outer_pane.set_position(outer_w / 2);
                outer_split = true;
            }
            // Let the re-layout triggered by the outer split allocate the
            // inner paned before we look at its width.
            return glib::ControlFlow::Continue;
        }
        let inner_w = inner_pane.width();
        if inner_w > 100 {
            inner_pane.set_position(inner_w / 2);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    // `set_margin_all` (RelmWidgetExt) and the `adw` alias used by the
    // probe live in relm4's prelude (the libadwaita feature); the rest of
    // the widget traits come in through `super`'s `gtk::prelude`.
    use relm4::prelude::*;

    /// Build the three-pane layout (sidebar | note list | editor) with the
    /// same widget types and size requests as `init`, wired with
    /// [`wire_initial_split`], inside a plain window.
    fn build_probe_window() -> (gtk::Window, gtk::Paned, gtk::Paned) {
        let window = gtk::Window::new();

        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.set_width_request(230);
        sidebar.set_margin_all(8);
        sidebar.append(&gtk::SearchEntry::new());
        sidebar.append(&gtk::ListBox::new());

        let middle = gtk::Box::new(gtk::Orientation::Vertical, 6);
        middle.set_width_request(300);
        middle.set_margin_all(8);
        middle.append(&gtk::Label::new(Some("notes")));
        middle.append(&gtk::ListBox::new());

        let editor = gtk::Box::new(gtk::Orientation::Vertical, 0);
        editor.set_margin_all(8);
        let buf = sourceview5::Buffer::new(None);
        let view = sourceview5::View::new();
        view.set_buffer(Some(&buf));
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        scroll.set_vexpand(true);
        editor.append(&scroll);

        let middle_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        middle_pane.set_start_child(Some(&sidebar));
        middle_pane.set_end_child(Some(&middle));
        let main_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        main_pane.set_start_child(Some(&middle_pane));
        main_pane.set_end_child(Some(&editor));
        wire_initial_split(&main_pane, &middle_pane);

        window.set_child(Some(&main_pane));
        (window, main_pane, middle_pane)
    }

    /// Run main-loop iterations for `ms` milliseconds. A periodic ticker
    /// source is registered first so `iteration(true)` can never block on an
    /// empty event queue — it always has the ticker to dispatch, and idle
    /// sources (like `wire_initial_split`) get to run in between.
    fn pump_ms(ctx: &glib::MainContext, ms: u64) {
        let _ticker = glib::timeout_add_local(std::time::Duration::from_millis(5), || {
            glib::ControlFlow::Continue
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            ctx.iteration(true);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Run main-loop iterations until `cond` holds or the timeout elapses
    /// (see [`pump_ms`] for the ticker trick).
    fn pump_until(ctx: &glib::MainContext, cond: impl FnMut() -> bool, max_ms: u64) {
        let mut cond = cond;
        let _ticker = glib::timeout_add_local(std::time::Duration::from_millis(5), || {
            glib::ControlFlow::Continue
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_ms);
        while !cond() && std::time::Instant::now() < deadline {
            ctx.iteration(true);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// The panes must stay inside the window at any width: after the
    /// initial split is applied, resizing the window (smaller *and* larger,
    /// as a tiling WM does) must keep every divider within the content
    /// bounds, and a later divider drag must keep *its* proportions on the
    /// next resize. Regression test for the niri/Wayland report where the
    /// sidebar overflowed the window and then took over the whole screen on
    /// maximize.
    ///
    /// Manual probe: `cargo test --bin notas panes_stay_inside_window -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires a display"]
    fn panes_stay_inside_window() {
        gtk::init().expect("gtk init");
        adw::init().expect("adw init");

        let (window, main_pane, middle_pane) = build_probe_window();
        let ctx = glib::MainContext::default();

        window.set_default_size(960, 600);
        window.present();
        // Keep pumping until the window maps **and** the initial 1:1:2 split
        // lands (the idle applies it on its own turn, which may come several
        // iterations after the map).
        pump_until(
            &ctx,
            || {
                let w = window.width();
                w > 100
                    && main_pane.width() == w
                    && (main_pane.position() - w / 2).abs() <= 3
                    && (middle_pane.position() - main_pane.position() / 2).abs() <= 3
            },
            3000,
        );
        let mut win_w = window.width();
        assert!(
            win_w > 100,
            "window should map at a real width, got {win_w}"
        );
        // The paned fills the window horizontally and the initial 1:1:2
        // split is in place.
        assert_eq!(
            main_pane.width(),
            win_w,
            "paned must match the window width"
        );
        assert!(
            (main_pane.position() - win_w / 2).abs() <= 3,
            "outer divider {} not half of {win_w}",
            main_pane.position()
        );
        assert!((middle_pane.position() - main_pane.position() / 2).abs() <= 3);

        // Resize down, up, and down again (as a tiling WM does): dividers
        // must stay inside the window and keep the 1:1:2 proportions.
        for (w, h) in [(640, 500), (1920, 900), (500, 400)] {
            window.set_default_size(w, h);
            pump_ms(&ctx, 150);

            win_w = window.width();
            assert!(win_w > 100, "window should stay mapped, got {win_w}");
            let outer = main_pane.position();
            let inner = middle_pane.position();
            assert!(
                outer > 0 && outer <= win_w,
                "outer divider {outer} outside window {win_w}"
            );
            assert!(
                inner > 0 && inner <= outer,
                "inner divider {inner} outside its paned {outer}"
            );
            assert!(
                (outer - win_w / 2).abs() <= 3,
                "outer divider {outer} not half of {win_w}"
            );
            assert!(
                (inner - outer / 2).abs() <= 3,
                "inner divider {inner} not half of {outer}"
            );
        }

        // A user drag to 60% of the window must survive the next resize.
        main_pane.set_position(win_w * 3 / 5);
        pump_ms(&ctx, 150);
        let dragged = main_pane.position();
        assert!(dragged > win_w / 2, "drag did not move the divider");

        window.set_default_size(win_w * 2, 900);
        pump_ms(&ctx, 150);
        let outer = main_pane.position();
        let win_w2 = window.width();
        let expected = win_w2 * 3 / 5;
        assert!(
            (outer - expected).abs() <= 3,
            "dragged split lost on maximize: divider {outer}, expected {expected}"
        );
    }
}
