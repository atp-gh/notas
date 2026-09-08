//! GTK dialogs used by the application coordinator.

use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
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
