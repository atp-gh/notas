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

/// Build the settings window. `emit` forwards UI events to the app. The
/// window is modal and transient for the main window (the app presents it
/// against `self.widgets.window`), and is kept alive by the `Widgets`
/// struct so reopening it re-presents the same instance.
pub fn build_settings_window<E>(settings: &Settings, emit: E) -> adw::PreferencesDialog
where
    E: Fn(AppMsg) + 'static + Clone,
{
    let window = adw::PreferencesDialog::new();
    window.set_title(tr!("Settings"));
    window.set_content_width(440);

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

    // --- editor: line numbers, status bar ---------------------------------
    let editor_group = adw::PreferencesGroup::new();
    editor_group.set_title(tr!("Editor"));

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
    editor_group.add(&line_numbers_row);
    editor_group.add(&status_bar_row);
    page.add(&editor_group);
    window.add(&page);

    // --- sync: S3-compatible storage ------------------------------
    let sync_page = adw::PreferencesPage::new();
    sync_page.set_title(tr!("Sync"));

    let sync_group = adw::PreferencesGroup::new();
    sync_group.set_title(tr!("S3-compatible storage"));
    sync_group.set_description(Some(tr!(
        "Notes sync as Markdown files to an S3-compatible bucket \
         (AWS, Cloudflare R2, Backblaze B2, MinIO, …). Use a dedicated \
         access key restricted to this bucket."
    )));

    // adw::EntryRow / adw::PasswordEntryRow expose their text through the
    // GtkEditable interface (the Rust bindings only surface a few direct
    // methods), which is in scope via the gtk prelude.
    let endpoint_entry = adw::EntryRow::new();
    endpoint_entry.set_title(tr!("Endpoint (optional)"));
    endpoint_entry.set_text(&settings.sync.endpoint);
    {
        let emit = emit.clone();
        endpoint_entry.connect_changed(move |row| {
            emit(AppMsg::SyncEndpointChanged(row.text().to_string()));
        });
    }

    let region_entry = adw::EntryRow::new();
    region_entry.set_title(tr!("Region"));
    region_entry.set_text(&settings.sync.region);
    {
        let emit = emit.clone();
        region_entry.connect_changed(move |row| {
            emit(AppMsg::SyncRegionChanged(row.text().to_string()));
        });
    }

    let bucket_entry = adw::EntryRow::new();
    bucket_entry.set_title(tr!("Bucket"));
    bucket_entry.set_text(&settings.sync.bucket);
    {
        let emit = emit.clone();
        bucket_entry.connect_changed(move |row| {
            emit(AppMsg::SyncBucketChanged(row.text().to_string()));
        });
    }

    let prefix_entry = adw::EntryRow::new();
    prefix_entry.set_title(tr!("Key prefix"));
    prefix_entry.set_text(&settings.sync.prefix);
    {
        let emit = emit.clone();
        prefix_entry.connect_changed(move |row| {
            emit(AppMsg::SyncPrefixChanged(row.text().to_string()));
        });
    }

    let access_key_entry = adw::EntryRow::new();
    access_key_entry.set_title(tr!("Access key ID"));
    access_key_entry.set_text(&settings.sync.access_key_id);
    {
        let emit = emit.clone();
        access_key_entry.connect_changed(move |row| {
            emit(AppMsg::SyncAccessKeyChanged(row.text().to_string()));
        });
    }

    let secret_key_entry = adw::PasswordEntryRow::new();
    secret_key_entry.set_title(tr!("Secret access key"));
    secret_key_entry.set_text(&settings.sync.secret_access_key);
    {
        let emit = emit.clone();
        secret_key_entry.connect_changed(move |row| {
            emit(AppMsg::SyncSecretKeyChanged(row.text().to_string()));
        });
    }

    // Provider presets fill endpoint/region with common defaults; the
    // entries stay editable afterwards.
    let provider_model = gtk::StringList::new(&[
        tr!("Custom"),
        tr!("AWS S3"),
        tr!("Cloudflare R2"),
        tr!("Backblaze B2"),
        tr!("MinIO"),
    ]);
    let provider_row = adw::ComboRow::new();
    provider_row.set_title(tr!("Provider"));
    provider_row.set_subtitle(tr!("Fills the endpoint and region fields"));
    provider_row.set_model(Some(&provider_model));
    {
        let endpoint = endpoint_entry.clone();
        let region = region_entry.clone();
        provider_row.connect_selected_notify(move |row| {
            match row.selected() {
                // AWS: empty endpoint + default region.
                1 => {
                    endpoint.set_text("");
                    region.set_text("us-east-1");
                }
                // R2: region is `auto`; the endpoint is account-specific,
                // so only the region is filled.
                2 => region.set_text("auto"),
                // B2: default region; the endpoint is account-specific.
                3 => region.set_text("us-west-002"),
                // MinIO: typical local development default.
                4 => {
                    endpoint.set_text("http://localhost:9000");
                    region.set_text("us-east-1");
                }
                _ => {}
            }
        });
    }

    let last_synced_row = adw::ActionRow::new();
    last_synced_row.set_title(tr!("Last synced"));
    let last_synced = if settings.sync.last_synced_at.is_empty() {
        tr!("Never").to_string()
    } else {
        settings.sync.last_synced_at.clone()
    };
    last_synced_row.set_subtitle(&last_synced);

    sync_group.add(&provider_row);
    sync_group.add(&endpoint_entry);
    sync_group.add(&region_entry);
    sync_group.add(&bucket_entry);
    sync_group.add(&prefix_entry);
    sync_group.add(&access_key_entry);
    sync_group.add(&secret_key_entry);
    sync_group.add(&last_synced_row);
    sync_page.add(&sync_group);
    window.add(&sync_page);

    window
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

        let window = build_settings_window(&settings, emit);
        // Window basics.
        assert_eq!(window.title(), "Settings");

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

        // Sync page: one provider combo + the theme combo, and entries
        // that emit their field-changed messages on edit.
        let mut combos = Vec::new();
        find_all::<adw::ComboRow>(&window, &mut combos);
        assert_eq!(combos.len(), 2, "theme + provider combo rows");
        assert_eq!(combos[0].title(), "Theme");
        assert_eq!(combos[1].title(), "Provider");

        let mut entries = Vec::new();
        find_all::<adw::EntryRow>(&window, &mut entries);
        let bucket = entries
            .iter()
            .find(|r| r.title() == "Bucket")
            .expect("bucket entry row");
        bucket.set_text("my-notes");
        assert!(matches!(
            pop(&messages),
            AppMsg::SyncBucketChanged(v) if v == "my-notes"
        ));

        let secret = find_row::<adw::PasswordEntryRow>(&window).expect("secret key row");
        secret.set_text("s3cr3t");
        assert!(matches!(
            pop(&messages),
            AppMsg::SyncSecretKeyChanged(v) if v == "s3cr3t"
        ));

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
