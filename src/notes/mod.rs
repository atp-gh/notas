//! GTK note-oriented widgets.
//!
//! Note list, notebook tree, and tag chips will live here. Keeping these
//! widgets separate from the application coordinator prevents GTK row
//! bookkeeping from leaking into core note operations.

use gtk::prelude::*;
use relm4::RelmWidgetExt;

/// Remove every child row from a note list.
pub(crate) fn clear_list(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

/// Remove every child chip from a flow container.
pub(crate) fn clear_flow(flow: &gtk::FlowBox) {
    while let Some(child) = flow.first_child() {
        flow.remove(&child);
    }
}

/// Build one note list row with a title and secondary metadata line.
pub(crate) fn row(title: &str, subtitle: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(true);
    let title_label = gtk::Label::new(None);
    title_label.set_xalign(0.0);
    title_label.set_ellipsize(pango::EllipsizeMode::End);
    title_label.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(title)));
    let sub_label = gtk::Label::new(None);
    sub_label.set_xalign(0.0);
    sub_label.set_ellipsize(pango::EllipsizeMode::End);
    sub_label.set_markup(&format!(
        "<span size='small' foreground='#8f8f8f'>{}</span>",
        glib::markup_escape_text(subtitle)
    ));
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
    vbox.set_margin_all(6);
    vbox.append(&title_label);
    vbox.append(&sub_label);
    row.set_child(Some(&vbox));
    row
}
