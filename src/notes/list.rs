//! GTK note-list rendering.

use std::cell::RefCell;

use gtk::prelude::*;

use notas::core::model::{Note, SearchHit};

/// Render regular or trashed notes into the note list.
pub(crate) fn render_notes(
    list: &gtk::ListBox,
    empty_label: &gtk::Label,
    row_ids: &RefCell<Vec<i64>>,
    notes: &[Note],
    row: impl Fn(&str, &str) -> gtk::ListBoxRow,
) {
    super::clear_list(list);
    let mut ids = row_ids.borrow_mut();
    ids.clear();
    empty_label.set_visible(notes.is_empty());
    for note in notes {
        ids.push(note.id.0);
        list.append(&row(&note.title, &note.updated_at));
    }
}

/// Render full-text search hits into the note list.
pub(crate) fn render_search(
    list: &gtk::ListBox,
    empty_label: &gtk::Label,
    row_ids: &RefCell<Vec<i64>>,
    hits: &[SearchHit],
    row: impl Fn(&str, &str) -> gtk::ListBoxRow,
) {
    super::clear_list(list);
    let mut ids = row_ids.borrow_mut();
    ids.clear();
    empty_label.set_visible(hits.is_empty());
    for hit in hits {
        ids.push(hit.id.0);
        list.append(&row(&hit.title, &hit.snippet));
    }
}
