//! GTK dialogs used by the application coordinator.

use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use relm4::RelmWidgetExt;
use relm4::Sender;

use super::protocol::AppMsg;
use crate::tr;

type AppSender = Sender<AppMsg>;

/// Ask how to handle the current unsaved note.
pub(crate) fn unsaved(window: &adw::ApplicationWindow, sender: &AppSender) {
    let dialog = adw::AlertDialog::new(
        Some(tr!("Unsaved changes")),
        Some(tr!("The current note has unsaved changes.")),
    );
    dialog.add_response("cancel", tr!("Cancel"));
    dialog.add_response("discard", tr!("Discard"));
    dialog.add_response("save", tr!("Save"));
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    let sender = sender.clone();
    dialog.connect_response(None::<&str>, move |_dialog, response| {
        let message = match response {
            "save" => AppMsg::DialogSave,
            "discard" => AppMsg::DialogDiscard,
            _ => AppMsg::DialogCancel,
        };
        let _ = sender.send(message);
    });
    dialog.present(Some(window));
}

/// Show a confirmation dialog whose confirm button carries a custom label.
pub(crate) fn confirm_action(
    window: &adw::ApplicationWindow,
    sender: &AppSender,
    title: &str,
    body: &str,
    ok_label: &str,
    message: AppMsg,
) {
    let dialog = adw::AlertDialog::new(Some(title), Some(body));
    dialog.add_response("cancel", tr!("Cancel"));
    dialog.add_response("confirm", ok_label);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let sender = sender.clone();
    dialog.connect_response(None::<&str>, move |_dialog, response| {
        if response == "confirm" {
            let _ = sender.send(message.clone());
        }
    });
    dialog.present(Some(window));
}

/// Show a destructive confirmation dialog.
pub(crate) fn confirm(
    window: &adw::ApplicationWindow,
    sender: &AppSender,
    title: &str,
    body: &str,
    message: AppMsg,
) {
    confirm_action(window, sender, title, body, tr!("Delete"), message);
}

/// Show an error message owned by the current window.
pub(crate) fn error(window: &adw::ApplicationWindow, message: &str) {
    let dialog = adw::AlertDialog::new(Some(tr!("Error")), Some(message));
    dialog.add_response("ok", tr!("OK"));
    dialog.set_default_response(Some("ok"));
    dialog.present(Some(window));
}

/// Prompt for a short text value and emit a semantic application message.
#[expect(
    deprecated,
    reason = "gtk::Dialog is deprecated since 4.10; keep until adw::AlertDialog can host custom content"
)]
pub(crate) fn input(
    window: &adw::ApplicationWindow,
    sender: &AppSender,
    title: &str,
    placeholder: &str,
    initial: &str,
    ok: impl Fn(String) -> AppMsg + 'static,
) {
    let dialog = gtk::Dialog::with_buttons(
        Some(title),
        Some(window),
        gtk::DialogFlags::MODAL,
        &[
            (tr!("Cancel"), gtk::ResponseType::Cancel),
            (tr!("OK"), gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Ok);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some(placeholder));
    entry.set_text(initial);
    entry.set_activates_default(true);
    dialog.content_area().append(&entry);
    dialog.set_size_request(360, -1);
    let sender = sender.clone();
    let entry_for_response = entry.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Ok {
            let _ = sender.send(ok(entry_for_response.text().to_string()));
        }
        dialog.close();
    });
    dialog.present();
    entry.grab_focus();
}

/// List one note's attachments with Open/Delete actions.
///
/// `items` carries `(uuid, filename, size)` triples already filtered to the
/// ids the note references; opening resolves `resources_dir/<uuid>` to a
/// `file://` URI and asks the desktop to open it. Deleting emits
/// [`AppMsg::DeleteAttachment`] (row + blob go away, the editor link is
/// cleaned by the event handler).
#[expect(
    deprecated,
    reason = "gtk::Dialog is deprecated since 4.10; keep until adw::AlertDialog can host custom content"
)]
pub(crate) fn attachments(
    window: &adw::ApplicationWindow,
    sender: &AppSender,
    resources_dir: &std::path::Path,
    items: Vec<(String, String, i64)>,
) {
    let dialog = gtk::Dialog::with_buttons(
        Some(tr!("Attachments")),
        Some(window),
        gtk::DialogFlags::MODAL,
        &[(tr!("Close"), gtk::ResponseType::Close)],
    );
    let list = gtk::ListBox::new();
    if items.is_empty() {
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&gtk::Label::new(Some(tr!(
            "No attachments in this note"
        )))));
        list.append(&row);
    }
    for (uuid, filename, size) in items {
        let row = gtk::ListBoxRow::new();
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_all(6);
        let label = gtk::Label::new(None);
        label.set_markup(&format!(
            "<b>{}</b> <span alpha=\"55%\">({})</span>",
            glib::markup_escape_text(&filename),
            human_size(size)
        ));
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(true);
        hbox.append(&label);
        let open_btn = gtk::Button::with_label(tr!("Open"));
        {
            let dir = resources_dir.to_path_buf();
            let uuid = uuid.clone();
            open_btn.connect_clicked(move |_| {
                let path = dir.join(&uuid);
                let uri = gtk::gio::File::for_path(&path).uri();
                let _ = gtk::gio::AppInfo::launch_default_for_uri(
                    &uri,
                    None::<&gtk::gio::AppLaunchContext>,
                );
            });
        }
        hbox.append(&open_btn);
        let del_btn = gtk::Button::with_label(tr!("Delete"));
        {
            let sender = sender.clone();
            del_btn.connect_clicked(move |_| {
                let _ = sender.send(AppMsg::DeleteAttachment(uuid.clone()));
            });
        }
        hbox.append(&del_btn);
        row.set_child(Some(&hbox));
        list.append(&row);
    }
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_min_content_height(200);
    scroll.set_min_content_width(420);
    scroll.set_child(Some(&list));
    dialog.content_area().append(&scroll);
    let sender = sender.clone();
    dialog.connect_response(move |dialog, _| {
        dialog.close();
        let _ = &sender;
    });
    dialog.present();
}

/// Human-readable byte count for the attachments dialog.
fn human_size(size: i64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut value = size.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", size.max(0), UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
