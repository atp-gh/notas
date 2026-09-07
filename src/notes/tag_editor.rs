//! Tag chips rendered alongside the current note editor.

use gtk::prelude::*;

use crate::core::AppMsg;
use notas::domain_notes::Tag;

/// Rebuild removable tag chips for the selected note.
pub(crate) fn render(
    flow: &gtk::FlowBox,
    tags: &[Tag],
    current_note: Option<i64>,
    sender: &relm4::Sender<AppMsg>,
) {
    super::clear_flow(flow);
    let names: Vec<String> = tags.iter().map(|tag| tag.name.clone()).collect();
    for tag in tags {
        let chip = gtk::Button::with_label(&format!("× {}", tag.name));
        chip.add_css_class("pill");
        let sender = sender.clone();
        let names = names.clone();
        let tag_name = tag.name.clone();
        chip.connect_clicked(move |_| {
            if current_note.is_some() {
                let remaining: Vec<String> = names
                    .iter()
                    .filter(|name| **name != tag_name)
                    .cloned()
                    .collect();
                let _ = sender.send(AppMsg::TagsEdited(remaining));
            }
        });
        flow.append(&chip);
    }
}
