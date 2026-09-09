//! GTK note-oriented widgets.
//!
//! Note list, notebook tree, and tag chips will live here. Keeping these
//! widgets separate from the application coordinator prevents GTK row
//! bookkeeping from leaking into core note operations.

pub(crate) mod list;
pub(crate) mod notebook;
pub(crate) mod sidebar;
pub(crate) mod tag_editor;

/// What the in-app copy/cut buffer holds: one notebook or one note.
///
/// Kept in memory only (never touches the system clipboard): `Copy`
/// duplicates on paste and stays armed for repeated pastes, `Cut` moves
/// once and disarms. Small enough (`Copy`) to pass by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipboardKind {
    /// A single note.
    Note,
    /// A notebook subtree.
    Notebook,
}

/// One buffered copy/cut entry for right-click paste.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Clipboard {
    /// Whether the buffered row is a note or a notebook.
    pub kind: ClipboardKind,
    /// Raw row id (`NoteId.0` / `NotebookId.0`; raw so menus stay GTK-plain).
    pub id: i64,
    /// `true` = cut (move once, then disarm), `false` = copy (repeatable).
    pub cut: bool,
}

use gtk::prelude::*;
use relm4::RelmWidgetExt;

/// Remove every note row from a note list.
///
/// Only `ListBoxRow`s are removed: the right-click popover is parented to
/// the list itself (it outlives rebuilds), and `remove()` on it would fail
/// with "Tried to remove non-child" while `first_child()` keeps returning
/// it — an infinite loop on every refresh.
pub(crate) fn clear_list(list: &gtk::ListBox) {
    let mut rows = Vec::new();
    let mut child = list.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if widget.downcast_ref::<gtk::ListBoxRow>().is_some() {
            rows.push(widget);
        }
    }
    for row in rows {
        list.remove(&row);
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
