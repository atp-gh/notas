//! The Sync page of the settings dialog: backend selection with the
//! per-backend credential rows, the last-synced readout, and the
//! end-to-end encryption switch with its password confirmation dialog.

use std::cell::Cell;
use std::rc::Rc;

use libadwaita as adw;
use libadwaita::prelude::*;

use crate::application::config::{Settings, SyncType};
use crate::tr;
use crate::ui::AppMsg;

/// Build the "Sync" page. Changing a row emits the matching `AppMsg` with
/// **immediate effect** (no Apply button), which the app applies to the
/// live UI and saves.
pub fn build_sync_page<E>(settings: &Settings, emit: E) -> adw::PreferencesPage
where
    E: Fn(AppMsg) + 'static + Clone,
{
    // --- sync: backend + target credentials -------------------------
    let sync_page = adw::PreferencesPage::new();
    sync_page.set_title(tr!("Sync"));
    sync_page.set_icon_name(Some("folder-remote-symbolic"));

    let sync_group = adw::PreferencesGroup::new();
    sync_group.set_title(tr!("Sync target"));
    sync_group.set_description(Some(tr!(
        "Notes sync as Markdown files to the backend selected above; the \
         fields shown belong to that backend."
    )));

    // Backend selector: both S3 and WebDAV are implemented, so a plain
    // combo row is enough; the row sets below are shown per selection.
    let type_model = gtk::StringList::new(&[tr!("S3"), tr!("WebDAV")]);
    let type_row = adw::ComboRow::new();
    type_row.set_title(tr!("Sync type"));
    type_row.set_subtitle(tr!("Backend used to sync notes"));
    type_row.set_model(Some(&type_model));
    type_row.set_selected(settings.sync.kind.index());

    // adw::EntryRow / adw::PasswordEntryRow expose their text through the
    // GtkEditable interface (the Rust bindings only surface a few direct
    // methods), which is in scope via the gtk prelude.
    let mut s3_rows: Vec<gtk::Widget> = Vec::new();

    let endpoint_entry = adw::EntryRow::new();
    endpoint_entry.set_title(tr!("Endpoint (optional)"));
    endpoint_entry.set_text(&settings.sync.s3.endpoint);
    {
        let emit = emit.clone();
        endpoint_entry.connect_changed(move |row| {
            emit(AppMsg::SyncEndpointChanged(row.text().to_string()));
        });
    }
    s3_rows.push(endpoint_entry.upcast::<gtk::Widget>());

    let region_entry = adw::EntryRow::new();
    region_entry.set_title(tr!("Region"));
    region_entry.set_text(&settings.sync.s3.region);
    {
        let emit = emit.clone();
        region_entry.connect_changed(move |row| {
            emit(AppMsg::SyncRegionChanged(row.text().to_string()));
        });
    }
    s3_rows.push(region_entry.upcast::<gtk::Widget>());

    let bucket_entry = adw::EntryRow::new();
    bucket_entry.set_title(tr!("Bucket"));
    bucket_entry.set_text(&settings.sync.s3.bucket);
    {
        let emit = emit.clone();
        bucket_entry.connect_changed(move |row| {
            emit(AppMsg::SyncBucketChanged(row.text().to_string()));
        });
    }
    s3_rows.push(bucket_entry.upcast::<gtk::Widget>());

    let prefix_entry = adw::EntryRow::new();
    prefix_entry.set_title(tr!("Key prefix"));
    prefix_entry.set_text(&settings.sync.s3.prefix);
    {
        let emit = emit.clone();
        prefix_entry.connect_changed(move |row| {
            emit(AppMsg::SyncPrefixChanged(row.text().to_string()));
        });
    }
    s3_rows.push(prefix_entry.upcast::<gtk::Widget>());

    let access_key_entry = adw::EntryRow::new();
    access_key_entry.set_title(tr!("Access key ID"));
    access_key_entry.set_text(&settings.sync.s3.access_key_id);
    {
        let emit = emit.clone();
        access_key_entry.connect_changed(move |row| {
            emit(AppMsg::SyncAccessKeyChanged(row.text().to_string()));
        });
    }
    s3_rows.push(access_key_entry.upcast::<gtk::Widget>());

    let secret_key_entry = adw::PasswordEntryRow::new();
    secret_key_entry.set_title(tr!("Secret access key"));
    secret_key_entry.set_text(&settings.sync.s3.secret_access_key);
    {
        let emit = emit.clone();
        secret_key_entry.connect_changed(move |row| {
            emit(AppMsg::SyncSecretKeyChanged(row.text().to_string()));
        });
    }
    s3_rows.push(secret_key_entry.upcast::<gtk::Widget>());

    let mut webdav_rows: Vec<gtk::Widget> = Vec::new();

    let url_entry = adw::EntryRow::new();
    url_entry.set_title(tr!("Server URL"));
    url_entry.set_text(&settings.sync.webdav.url);
    {
        let emit = emit.clone();
        url_entry.connect_changed(move |row| {
            emit(AppMsg::SyncUrlChanged(row.text().to_string()));
        });
    }
    webdav_rows.push(url_entry.upcast::<gtk::Widget>());

    let directory_entry = adw::EntryRow::new();
    directory_entry.set_title(tr!("Directory"));
    directory_entry.set_text(&settings.sync.webdav.directory);
    {
        let emit = emit.clone();
        directory_entry.connect_changed(move |row| {
            emit(AppMsg::SyncDirectoryChanged(row.text().to_string()));
        });
    }
    webdav_rows.push(directory_entry.upcast::<gtk::Widget>());

    let username_entry = adw::EntryRow::new();
    username_entry.set_title(tr!("Username"));
    username_entry.set_text(&settings.sync.webdav.username);
    {
        let emit = emit.clone();
        username_entry.connect_changed(move |row| {
            emit(AppMsg::SyncUsernameChanged(row.text().to_string()));
        });
    }
    webdav_rows.push(username_entry.upcast::<gtk::Widget>());

    let password_entry = adw::PasswordEntryRow::new();
    password_entry.set_title(tr!("Password"));
    password_entry.set_text(&settings.sync.webdav.password);
    {
        let emit = emit.clone();
        password_entry.connect_changed(move |row| {
            emit(AppMsg::SyncPasswordChanged(row.text().to_string()));
        });
    }
    webdav_rows.push(password_entry.upcast::<gtk::Widget>());

    let insecure_row = adw::SwitchRow::new();
    insecure_row.set_title(tr!("Allow insecure TLS"));
    insecure_row.set_subtitle(tr!(
        "Accept self-signed certificates and plain http; only for trusted \
         servers"
    ));
    insecure_row.set_active(settings.sync.webdav.insecure_tls);
    {
        let emit = emit.clone();
        insecure_row.connect_active_notify(move |row| {
            emit(AppMsg::SyncInsecureTlsChanged(row.is_active()));
        });
    }
    webdav_rows.push(insecure_row.upcast::<gtk::Widget>());

    let last_synced_row = adw::ActionRow::new();
    last_synced_row.set_title(tr!("Last synced"));
    let last_synced = if settings.sync.last_synced_at.is_empty() {
        tr!("Never").to_string()
    } else {
        settings.sync.last_synced_at.clone()
    };
    last_synced_row.set_subtitle(&last_synced);

    // Only the active backend's rows are visible; switching the combo
    // swaps them live (each row keeps editing its own section of the
    // settings, so nothing is lost when toggling back and forth).
    {
        let s3 = s3_rows.clone();
        let webdav = webdav_rows.clone();
        let emit = emit.clone();
        type_row.connect_selected_notify(move |row| {
            let kind = SyncType::from_index(row.selected());
            show_backend_rows(kind, &s3, &webdav);
            emit(AppMsg::SyncTypeChanged(kind));
        });
    }

    sync_group.add(&type_row);
    for row in &s3_rows {
        sync_group.add(row);
    }
    for row in &webdav_rows {
        sync_group.add(row);
    }
    sync_group.add(&last_synced_row);
    sync_page.add(&sync_group);

    // --- sync: end-to-end encryption -------------------------------
    // One global switch + password pair shared by both backends. The rows
    // stay visible when disabled (they are inert then) so the password can
    // be entered before flipping the switch on.
    let enc_group = adw::PreferencesGroup::new();
    enc_group.set_title(tr!("Encryption"));
    enc_group.set_description(Some(tr!(
        "Encrypt every note before it is uploaded, so the backend only ever \
         stores ciphertext. Enter the same password on every device that \
         syncs."
    )));

    let enc_switch = adw::SwitchRow::new();
    enc_switch.set_title(tr!("Encrypt synced notes"));
    enc_switch.set_subtitle(tr!(
        "Backend files are sealed (XChaCha20-Poly1305 + Argon2id)"
    ));

    let enc_password = adw::PasswordEntryRow::new();
    enc_password.set_title(tr!("Encryption password"));

    let enc_confirm = adw::PasswordEntryRow::new();
    enc_confirm.set_title(tr!("Confirm password"));

    // The password row persists to the settings; the confirm row only
    // flags a mismatch live (immediate-effect dialog, no Apply button).
    {
        let emit = emit.clone();
        enc_password.connect_changed(move |row| {
            emit(AppMsg::SyncEncryptionPasswordChanged(
                row.text().to_string(),
            ));
        });
    }
    {
        let password_row = enc_password.clone();
        enc_confirm.connect_changed(move |row| {
            if row.text() != password_row.text() {
                row.add_css_class("error");
            } else {
                row.remove_css_class("error");
            }
        });
    }

    // The password stored when the dialog was built, used to confirm a
    // disable (typing it proves the user knows the key before the backend
    // is re-exposed as plaintext).
    let stored_password = settings.sync.encryption.password.clone();
    // Suppresses the notify handler while we flip the switch ourselves.
    let guard = Rc::new(Cell::new(false));
    {
        let guard = guard.clone();
        let password_row = enc_password.clone();
        let switch = enc_switch.clone();
        enc_switch.connect_active_notify(move |row| {
            let on = row.is_active();
            if guard.get() {
                return;
            }
            if on {
                // Enforce the minimum here so a weak password can't be
                // enabled in the first place; the sync engine enforces it
                // again as the source of truth.
                if password_row.text().len() < crate::sync::crypto::MIN_PASSWORD_LEN {
                    guard.set(true);
                    row.set_active(false);
                    guard.set(false);
                    error_alert(
                        tr!("Encryption password too short"),
                        tr!("The encryption password must be at least 8 characters."),
                    );
                    return;
                }
                emit(AppMsg::SyncEncryptionEnabledChanged(true));
            } else if !stored_password.is_empty() {
                // Keep the switch on until the current password is entered.
                guard.set(true);
                row.set_active(true);
                guard.set(false);
                confirm_disable_encryption(
                    &stored_password,
                    emit.clone(),
                    switch.clone(),
                    guard.clone(),
                );
            } else {
                emit(AppMsg::SyncEncryptionEnabledChanged(false));
            }
        });
    }

    enc_group.add(&enc_switch);
    enc_group.add(&enc_password);
    enc_group.add(&enc_confirm);
    sync_page.add(&enc_group);

    // Reflect the configured kind after the rows are attached.
    show_backend_rows(settings.sync.kind, &s3_rows, &webdav_rows);

    // Initialize the switch without firing the handler (the initial value
    // is a programmatic set, not a user action).
    guard.set(true);
    enc_switch.set_active(settings.sync.encryption.enabled);
    guard.set(false);

    sync_page
}

/// Show the rows of the selected backend and hide the other set.
fn show_backend_rows(kind: SyncType, s3_rows: &[gtk::Widget], webdav_rows: &[gtk::Widget]) {
    for row in s3_rows {
        row.set_visible(kind == SyncType::S3);
    }
    for row in webdav_rows {
        row.set_visible(kind == SyncType::WebDAV);
    }
}

/// Small alert dialog for settings validation failures.
fn error_alert(title: &str, body: &str) {
    let dialog = adw::AlertDialog::new(Some(title), Some(body));
    dialog.add_response("ok", tr!("OK"));
    dialog.set_default_response(Some("ok"));
    dialog.present(None::<&gtk::Window>);
}

/// Ask for the current encryption password before disabling encryption:
/// disabling re-uploads everything to the backend in plaintext, so the
/// toggle alone must not be able to expose the notes. On a wrong answer
/// the switch simply stays on.
#[expect(
    deprecated,
    reason = "gtk::Dialog is deprecated since 4.10; keep until adw::AlertDialog can host custom content"
)]
fn confirm_disable_encryption(
    stored: &str,
    emit: impl Fn(AppMsg) + 'static + Clone,
    switch: adw::SwitchRow,
    guard: Rc<Cell<bool>>,
) {
    let dialog = gtk::Dialog::with_buttons(
        Some(tr!("Disable encryption")),
        None::<&gtk::Window>,
        gtk::DialogFlags::MODAL,
        &[
            (tr!("Cancel"), gtk::ResponseType::Cancel),
            (tr!("Disable"), gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Ok);
    let entry = gtk::PasswordEntry::new();
    entry.set_placeholder_text(Some(tr!("Encryption password")));
    entry.set_activates_default(true);
    dialog.content_area().append(&entry);
    dialog.set_size_request(360, -1);
    let stored = stored.to_string();
    let entry2 = entry.clone();
    dialog.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Ok && entry2.text() == stored {
            guard.set(true);
            switch.set_active(false);
            guard.set(false);
            emit(AppMsg::SyncEncryptionEnabledChanged(false));
        } else if resp == gtk::ResponseType::Ok {
            error_alert(
                tr!("Wrong password"),
                tr!("The password does not match the one used to encrypt this backend."),
            );
        }
        d.close();
    });
    dialog.present();
    entry.grab_focus();
}
