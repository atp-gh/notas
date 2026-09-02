//! Settings dialog: an `adw::PreferencesDialog` opened from the app menu
//! (`PreferencesWindow` is the deprecated name since libadwaita 1.6).
//!
//! Rows edit the persisted [`crate::config::Settings`] with **immediate
//! effect** — changing a row sends an `AppMsg` to the app, which applies
//! the change to the live UI and saves the settings file. There is no
//! "Apply" button, matching GNOME conventions.

use libadwaita as adw;
use libadwaita::prelude::*;

use crate::app::AppMsg;
use crate::config::{Settings, ThemeMode};
use crate::tr;

/// Widgets the app needs to reach after building (to refresh the font row
/// when the font changes elsewhere).
pub struct SettingsWindow {
    pub window: adw::PreferencesDialog,
    pub font_row: adw::ActionRow,
    pub reset_font_btn: gtk::Button,
}

/// Subtitle for the font row: the font's own description, or a "default"
/// placeholder when no custom font is set.
pub fn font_subtitle(font_desc: Option<&str>) -> String {
    match font_desc {
        Some(desc) => {
            let shown = pango::FontDescription::from_string(desc).to_str();
            if shown.is_empty() {
                tr!("Default font").to_owned()
            } else {
                shown.to_string()
            }
        }
        None => tr!("Default font").to_owned(),
    }
}

/// Build the settings window. `emit` forwards UI events to the app. The
/// window is modal and transient for `main_window`, and is kept alive by
/// the `Widgets` struct so reopening it re-presents the same instance.
pub fn build_settings_window<E>(
    main_window: &impl IsA<gtk::Window>,
    settings: &Settings,
    emit: E,
) -> SettingsWindow
where
    E: Fn(AppMsg) + 'static + Clone,
{
    let window = adw::PreferencesDialog::new();
    window.set_title(tr!("Settings"));
    window.set_content_width(440);
    // Owned clone so the `'static` closures below can reach the parent.
    let main_window = main_window.clone();

    let page = adw::PreferencesPage::new();
    page.set_title(tr!("Appearance"));

    // --- appearance: theme ------------------------------------------------
    let theme_group = adw::PreferencesGroup::new();
    theme_group.set_title(tr!("Theme"));
    theme_group.set_description(Some(tr!(
        "Color scheme used for the whole app, including the editor."
    )));

    let model = gtk::StringList::new(&[tr!("Follow system"), tr!("Light"), tr!("Dark")]);
    let theme_row = adw::ComboRow::new();
    theme_row.set_title(tr!("Theme"));
    theme_row.set_model(Some(&model));
    theme_row.set_selected(settings.theme.mode.index());
    {
        let emit = emit.clone();
        theme_row.connect_selected_notify(move |row| {
            emit(AppMsg::ThemeChanged(ThemeMode::from_index(row.selected())));
        });
    }

    // --- editor: font, line numbers, status bar ---------------------------
    let editor_group = adw::PreferencesGroup::new();
    editor_group.set_title(tr!("Editor"));

    let initial_font = settings.editor.font_desc.clone();
    let font_row = adw::ActionRow::new();
    font_row.set_title(tr!("Editor font"));
    font_row.set_subtitle(&font_subtitle(settings.editor.font_desc.as_deref()));
    {
        let emit = emit.clone();
        font_row.connect_activated(move |_| {
            // Clone for the `FnOnce` choose-font callback: the dialog runs
            // asynchronously and outlives this activated handler.
            let emit = emit.clone();
            let dialog = gtk::FontDialog::new();
            dialog.set_title(tr!("Choose editor font"));
            let initial = initial_font
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(pango::FontDescription::from_string);
            // `choose_font` runs its callback once the dialog is dismissed;
            // `Ok(font)` means the user picked one, `Err` means cancelled.
            dialog.choose_font(
                Some(&main_window),
                initial.as_ref(),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    if let Ok(font) = result {
                        emit(AppMsg::FontChanged(Some(font.to_str().to_string())));
                    }
                },
            );
        });
    }

    let reset_font_btn = gtk::Button::with_label(tr!("Default"));
    reset_font_btn.set_tooltip_text(Some(tr!("Reset to the default font")));
    reset_font_btn.set_visible(settings.editor.font_desc.is_some());
    {
        let emit = emit.clone();
        reset_font_btn.connect_clicked(move |_| emit(AppMsg::FontChanged(None)));
    }
    font_row.add_suffix(&reset_font_btn);

    let line_numbers_row = adw::SwitchRow::new();
    line_numbers_row.set_title(tr!("Show line numbers"));
    line_numbers_row.set_subtitle(tr!("Gutter on the left of the editor"));
    line_numbers_row.set_active(settings.editor.show_line_numbers);
    {
        let emit = emit.clone();
        line_numbers_row.connect_active_notify(move |row| {
            emit(AppMsg::ToggleLineNumbers(row.is_active()));
        });
    }

    let status_bar_row = adw::SwitchRow::new();
    status_bar_row.set_title(tr!("Show status bar"));
    status_bar_row.set_subtitle(tr!("Saved-state indicator at the bottom of the editor"));
    status_bar_row.set_active(settings.interface.show_status_bar);
    {
        let emit = emit.clone();
        status_bar_row.connect_active_notify(move |row| {
            emit(AppMsg::ToggleStatusBar(row.is_active()));
        });
    }

    theme_group.add(&theme_row);
    page.add(&theme_group);
    editor_group.add(&font_row);
    editor_group.add(&line_numbers_row);
    editor_group.add(&status_bar_row);
    page.add(&editor_group);
    window.add(&page);

    SettingsWindow {
        window,
        font_row,
        reset_font_btn,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// Depth-first search for the first widget of type `W` in the tree.
    fn find_row<W>(widget: &impl IsA<gtk::Widget>) -> Option<W>
    where
        W: glib::object::Cast + Clone + 'static + IsA<gtk::Widget>,
    {
        let widget = widget.upcast_ref::<gtk::Widget>();
        if let Some(found) = widget.downcast_ref::<W>() {
            return Some(found.clone());
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            if let Some(found) = find_row::<W>(&c) {
                return Some(found);
            }
            child = c.next_sibling();
        }
        None
    }

    /// Depth-first collection of every widget of type `W` in the tree.
    fn find_all<W>(widget: &impl IsA<gtk::Widget>, out: &mut Vec<W>)
    where
        W: glib::object::Cast + Clone + 'static + IsA<gtk::Widget>,
    {
        let widget = widget.upcast_ref::<gtk::Widget>();
        if let Some(found) = widget.downcast_ref::<W>() {
            out.push(found.clone());
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            find_all::<W>(&c, out);
            child = c.next_sibling();
        }
    }

    /// Manual probe: `cargo test --bin notas settings_window_probe -- --ignored --nocapture`.
    ///
    /// Builds the settings dialog headless, drives each row programmatically,
    /// and asserts the `AppMsg` each change emits — the same messages the
    /// app's message handler applies to the live UI and saves to disk.
    #[test]
    #[ignore]
    fn settings_window_probe() {
        gtk::init().expect("gtk init");
        adw::init().expect("adw init");

        // A plain `gtk::Window` keeps the probe free of GApplication
        // lifecycle constraints (ApplicationWindow requires a started app).
        let main = gtk::Window::new();
        let settings = Settings::default();
        let messages: Rc<RefCell<Vec<AppMsg>>> = Rc::new(RefCell::new(Vec::new()));
        let emit = {
            let messages = messages.clone();
            move |msg| messages.borrow_mut().push(msg)
        };

        let SettingsWindow {
            window,
            font_row,
            reset_font_btn,
        } = build_settings_window(&main, &settings, emit);

        // Window basics.
        assert_eq!(window.title(), "Settings");
        assert_eq!(font_row.title(), "Editor font");
        assert_eq!(font_subtitle(None), "Default font");
        assert_eq!(font_row.subtitle().as_deref(), Some("Default font"));
        assert!(
            !reset_font_btn.is_visible(),
            "reset hidden without a custom font"
        );

        // Realize the dialog so its pages/groups attach as widget children.
        window.present(Some(&main));
        let ctx = glib::MainContext::default();
        for _ in 0..20 {
            ctx.iteration(false);
        }

        let theme_row = find_row::<adw::ComboRow>(&window).expect("theme ComboRow in the dialog");
        let mut switches = Vec::new();
        find_all::<adw::SwitchRow>(&window, &mut switches);
        assert_eq!(switches.len(), 2, "line numbers + status bar switches");
        let lines = switches
            .iter()
            .find(|s| s.title() == "Show line numbers")
            .expect("line numbers switch");
        let bar = switches
            .iter()
            .find(|s| s.title() == "Show status bar")
            .expect("status bar switch");

        // AdwComboRow defers the selection-to-notify hop to the main loop
        // (its internal list model binding is created lazily), so pump with
        // blocking iterations until the deferred callback has run. Repeated
        // changes after the first are dropped by that lazy binding, which is
        // fine here: the index->mode mapping itself is unit-tested in
        // `config.rs`.
        let pump = || {
            let ctx = glib::MainContext::default();
            for _ in 0..30 {
                ctx.iteration(true);
            }
        };

        // Theme: switching the combo row emits ThemeChanged with the mode.
        // Reading `model()` materializes the row's lazy binding to its
        // selection model, which is what actually propagates a selection
        // change; without it the deferred hop never fires.
        let _ = theme_row.model();
        theme_row.set_selected(2);
        pump();
        assert!(matches!(
            pop(&messages),
            AppMsg::ThemeChanged(ThemeMode::Dark)
        ));
        // The row itself reflects the value we set.
        assert_eq!(theme_row.selected(), 2);

        // Switches: flipping active emits the matching toggle message right
        // away (SwitchRow's active-notify fires synchronously).
        lines.set_active(true);
        assert!(matches!(pop(&messages), AppMsg::ToggleLineNumbers(true)));
        bar.set_active(false);
        assert!(matches!(pop(&messages), AppMsg::ToggleStatusBar(false)));

        // Font reset button emits FontChanged(None); the row stays wired.
        // (`Button::activate` does not emit `clicked` in GTK4; use the
        // explicit emitter, which a real press would trigger.)
        reset_font_btn.emit_clicked();
        assert!(matches!(pop(&messages), AppMsg::FontChanged(None)));

        assert!(
            messages.borrow().is_empty(),
            "no stray messages: {:?}",
            *messages.borrow()
        );
    }

    fn pop(messages: &Rc<RefCell<Vec<AppMsg>>>) -> AppMsg {
        let mut queue = messages.borrow_mut();
        assert!(
            !queue.is_empty(),
            "no message was emitted; queue: {:?}",
            *queue
        );
        queue.remove(0)
    }
}
