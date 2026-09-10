//! Settings dialog: an `adw::PreferencesDialog` opened from the app menu
//! (`PreferencesWindow` is the deprecated name since libadwaita 1.6).
//!
//! The dialog assembles the per-topic pages built by the [`appearance`]
//! and [`sync`] submodules. Rows edit the persisted
//! [`crate::application::config::Settings`] with **immediate effect** —
//! changing a row sends an `AppMsg` to the app, which applies the change
//! to the live UI and saves the settings file. There is no "Apply" button,
//! matching GNOME conventions.

mod appearance;
mod sync;

use libadwaita as adw;
use libadwaita::prelude::*;

use crate::application::config::Settings;
use crate::tr;
use crate::ui::AppMsg;

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

    window.add(&appearance::build_appearance_page(settings, emit.clone()));
    window.add(&sync::build_sync_page(settings, emit));

    window
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::application::config::{SyncType, ThemeMode};

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
    #[ignore = "manual probe: needs a display and visual inspection"]
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
        assert_eq!(
            switches.len(),
            4,
            "line numbers + status bar + insecure-TLS + encryption switches"
        );
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

        // Sync page: one sync-type combo + the theme combo, and rows that
        // emit their field-changed messages on edit.
        let mut combos = Vec::new();
        find_all::<adw::ComboRow>(&window, &mut combos);
        assert_eq!(combos.len(), 2, "theme + sync-type combo rows");
        assert_eq!(combos[0].title(), "Theme");
        assert_eq!(combos[1].title(), "Sync type");

        // Sync type: both backends are implemented and selectable; the row
        // reflects the configured kind and starts on S3.
        let type_row = &combos[1];
        assert_eq!(type_row.selected(), SyncType::S3.index());

        // Switching to WebDAV sticks (no reserved-backend snap-back) and
        // swaps the visible field rows.
        let _ = type_row.model();
        type_row.set_selected(SyncType::Webdav.index());
        pump();
        assert_eq!(type_row.selected(), SyncType::Webdav.index());
        assert!(matches!(
            pop(&messages),
            AppMsg::SyncTypeChanged(SyncType::Webdav)
        ));

        // Per-backend fields: S3 rows edit the s3 section, WebDAV rows the
        // webdav section. Rows are looked up by title because both row
        // sets always exist in the tree (only visibility switches).
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

        let url = entries
            .iter()
            .find(|r| r.title() == "Server URL")
            .expect("server URL row");
        url.set_text("https://nc.example/remote.php/dav/files/alice");
        assert!(matches!(
            pop(&messages),
            AppMsg::SyncUrlChanged(v) if v == "https://nc.example/remote.php/dav/files/alice"
        ));

        // Four password rows exist (S3 secret key + WebDAV password +
        // encryption password + confirm).
        let mut password_rows = Vec::new();
        find_all::<adw::PasswordEntryRow>(&window, &mut password_rows);
        assert_eq!(password_rows.len(), 4);
        let secret = password_rows
            .iter()
            .find(|r| r.title() == "Secret access key")
            .expect("secret key row");
        secret.set_text("s3cr3t");
        assert!(matches!(
            pop(&messages),
            AppMsg::SyncSecretKeyChanged(v) if v == "s3cr3t"
        ));
        let password = password_rows
            .iter()
            .find(|r| r.title() == "Password")
            .expect("webdav password row");
        password.set_text("app-pw");
        assert!(matches!(
            pop(&messages),
            AppMsg::SyncPasswordChanged(v) if v == "app-pw"
        ));

        // Encryption: entering a password emits its message; flipping the
        // switch on with a valid password enables encryption. (With an
        // empty password the switch snaps back and shows an alert instead
        // of emitting, which the probe avoids by filling the field first.)
        let enc_password = password_rows
            .iter()
            .find(|r| r.title() == "Encryption password")
            .expect("encryption password row");
        let enc_switch = switches
            .iter()
            .find(|s| s.title() == "Encrypt synced notes")
            .expect("encryption switch");
        enc_password.set_text("correct horse battery staple");
        assert!(matches!(
            pop(&messages),
            AppMsg::SyncEncryptionPasswordChanged(v) if v == "correct horse battery staple"
        ));
        enc_switch.set_active(true);
        assert!(matches!(
            pop(&messages),
            AppMsg::SyncEncryptionEnabledChanged(true)
        ));

        // The insecure-TLS switch emits its message.
        let insecure = switches
            .iter()
            .find(|s| s.title() == "Allow insecure TLS")
            .expect("insecure TLS switch");
        insecure.set_active(true);
        assert!(matches!(
            pop(&messages),
            AppMsg::SyncInsecureTlsChanged(true)
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
