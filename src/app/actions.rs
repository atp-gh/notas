//! Note-lifecycle actions and widget-rebuild helpers for the app
//! component.
//!
//! These `impl App` methods translate app state into worker requests
//! (`DbMsg`) and rebuild the note/notebook/tag widgets. The message
//! handlers in [`super::messages`] call them; keeping them here (instead
//! of inside `super`) leaves the coordinator's `init` and `update` lean.
//! They are `pub(super)` because they cross module boundaries within
//! [`super`] itself.

use gtk::prelude::*;
use relm4::ComponentController;

use notas::core::model::{NoteId, NotebookId, SearchHit};

use super::App;
use crate::application::DbMsg;
use crate::notes::{ClipboardKind, clear_flow, row as note_row};
use crate::tr;
use crate::ui::{AppMsg, ViewMode};

impl App {
    // ------------------------------------------------------------ actions

    pub(super) fn create_note_now(&mut self) {
        let notebook_id = match &self.mode {
            ViewMode::Notebook(id) => Some(*id),
            _ => None,
        };
        self.worker.emit(DbMsg::CreateNote(notebook_id));
    }

    pub(super) fn save_note(&mut self) {
        if let Some(id) = self.current_note {
            let title = self.current_title();
            let content = self.current_content();
            if title != self.saved_title || content != self.saved_content {
                self.worker.emit(DbMsg::UpdateNote { id, title, content });
            } else {
                self.resolve_saved();
            }
        }
    }

    pub(super) fn current_title(&self) -> String {
        self.widgets.title_entry.text().to_string()
    }

    pub(super) fn current_content(&self) -> String {
        let buffer = &self.widgets.editor.source_buffer;
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .to_string()
    }

    pub(super) fn resolve_saved(&mut self) {
        if self.pending_close {
            self.finish_close();
        } else if self.pending_new_note {
            self.pending_new_note = false;
            self.create_note_now();
        } else if let Some(id) = self.pending_open.take() {
            self.worker.emit(DbMsg::LoadNote(id));
        } else {
            self.dirty = false;
            self.set_dirty(false);
        }
    }

    pub(super) fn finish_close(&mut self) {
        self.allow_close.set(true);
        self.widgets.window.close();
    }

    pub(super) fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
        if dirty {
            self.widgets.dirty_label.set_text(tr!("● Unsaved changes"));
            self.widgets.dirty_label.add_css_class("error");
        } else {
            self.widgets.dirty_label.set_text(tr!("All changes saved"));
            self.widgets.dirty_label.remove_css_class("error");
        }
    }

    pub(super) fn refresh_current_list(&mut self) {
        match &self.mode {
            ViewMode::All => self.worker.emit(DbMsg::LoadAll),
            ViewMode::Unfiled => self.worker.emit(DbMsg::LoadUnfiled),
            ViewMode::Trash => self.worker.emit(DbMsg::LoadTrashed),
            ViewMode::Notebook(id) => self.worker.emit(DbMsg::LoadNotes(*id)),
            ViewMode::Tag(id) => self.worker.emit(DbMsg::LoadByTag(*id)),
            ViewMode::Search(q) => self.worker.emit(DbMsg::Search(q.clone())),
        }
    }

    pub(super) fn update_view_title(&self) {
        let title = match &self.mode {
            ViewMode::All => tr!("All notes").to_string(),
            ViewMode::Unfiled => tr!("Unfiled").to_string(),
            ViewMode::Trash => tr!("Trash").to_string(),
            ViewMode::Notebook(id) => self
                .notebooks
                .iter()
                .find(|n| n.id == *id)
                .map_or_else(|| tr!("Notebook").to_string(), |n| n.name.clone()),
            ViewMode::Tag(id) => self
                .tags
                .iter()
                .find(|t| t.id == *id)
                .map_or_else(|| tr!("Tag").to_string(), |t| format!("#{}", t.name)),
            ViewMode::Search(q) => format!("{}: {q}", tr!("Search")),
        };
        self.widgets.view_title.set_text(&title);
    }

    pub(super) fn update_trash_buttons(&self) {
        let trash = matches!(self.mode, ViewMode::Trash);
        self.widgets.restore_btn.set_visible(trash);
        self.widgets.delete_btn.set_visible(trash);
        self.widgets
            .restore_btn
            .set_sensitive(self.selected_trashed.is_some());
        self.widgets
            .delete_btn
            .set_sensitive(self.selected_trashed.is_some());
        // The note right-click menu mirrors the header: trash rows offer
        // restore/delete-forever, live rows offer copy/cut/paste/trash.
        self.widgets.note_normal_box.set_visible(!trash);
        self.widgets.note_trash_box.set_visible(trash);
    }

    /// Execute the pending copy/cut buffer against `target` (`None` = top
    /// level / unfiled). Copies stay armed for repeated pastes; cuts move
    /// once and then disarm.
    pub(super) fn paste_clipboard(&mut self, target: Option<NotebookId>) {
        let Some(entry) = *self.clipboard.borrow() else {
            return;
        };
        match (entry.kind, entry.cut) {
            (ClipboardKind::Note, false) => {
                self.worker.emit(DbMsg::DuplicateNote {
                    id: NoteId(entry.id),
                    target,
                });
            }
            (ClipboardKind::Note, true) => {
                if self.dirty && self.current_note == Some(NoteId(entry.id)) {
                    self.save_note();
                }
                self.worker.emit(DbMsg::MoveNote {
                    id: NoteId(entry.id),
                    target,
                });
                self.clipboard.borrow_mut().take();
                self.refresh_cut_dim();
            }
            (ClipboardKind::Notebook, false) => {
                self.worker.emit(DbMsg::DuplicateNotebook {
                    id: NotebookId(entry.id),
                    target,
                });
            }
            (ClipboardKind::Notebook, true) => {
                if self.dirty {
                    self.save_note();
                }
                self.worker.emit(DbMsg::MoveNotebook {
                    id: NotebookId(entry.id),
                    target,
                });
                self.clipboard.borrow_mut().take();
            }
        }
    }

    /// Whether `note` currently lives inside the `root` notebook subtree
    /// (used to clear the editor optimistically on subtree trash).
    pub(super) fn note_in_subtree(&self, note: NoteId, root: NotebookId) -> bool {
        let Some(notebook) = self
            .notes
            .iter()
            .find(|n| n.id == note)
            .and_then(|n| n.notebook_id)
        else {
            return false;
        };
        let mut current = Some(notebook);
        // Depth guard against a corrupt parent cycle.
        for _ in 0..64 {
            let Some(id) = current else {
                return false;
            };
            if id == root {
                return true;
            }
            current = self
                .notebooks
                .iter()
                .find(|n| n.id == id)
                .and_then(|n| n.parent_id);
        }
        false
    }

    /// Dim the cut note row (if any) so the pending move is visible.
    /// Notebook-tree dimming would need a per-row opacity model column on
    /// the shared `CellRendererText`; the tree keeps its current renderer
    /// and only the note list dims in V1.
    pub(super) fn refresh_cut_dim(&self) {
        let cut = match *self.clipboard.borrow() {
            Some(entry) if entry.cut && entry.kind == ClipboardKind::Note => Some(entry.id),
            _ => None,
        };
        let ids = self.row_ids.borrow();
        let mut child = self.widgets.notes_list.first_child();
        let mut index = 0_usize;
        while let Some(widget) = child {
            child = widget.next_sibling();
            // Count rows only: the context popover is parented to the list
            // itself and must not shift the row_ids mapping.
            let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>() else {
                continue;
            };
            let dimmed = cut.is_some_and(|cut| ids.get(index).is_some_and(|&row| row == cut));
            row.set_opacity(if dimmed { 0.5 } else { 1.0 });
            index += 1;
        }
    }

    // ------------------------------------------------------------ rebuilds

    pub(super) fn rebuild_notebook_tree(&self) {
        crate::notes::notebook::rebuild_tree(&self.widgets.notebook_store, &self.notebooks);
    }

    pub(super) fn rebuild_tag_flow(&self) {
        clear_flow(&self.widgets.tag_flow);
        let mut ids = self.tag_ids.borrow_mut();
        ids.clear();
        for tag in &self.tags {
            ids.push(tag.id.0);
            let btn = gtk::ToggleButton::with_label(&format!("{} ({})", tag.name, tag.note_count));
            {
                let suppress = self.suppress_tag_toggle.clone();
                let sender = self.ui_sender.clone();
                let id = tag.id;
                btn.connect_toggled(move |b| {
                    if suppress.get() {
                        return;
                    }
                    let _ = sender.send(if b.is_active() {
                        AppMsg::SelectTag(Some(id))
                    } else {
                        AppMsg::SelectTag(None)
                    });
                });
            }
            self.suppress_tag_toggle.set(true);
            btn.set_active(self.active_tag == Some(tag.id));
            self.suppress_tag_toggle.set(false);

            let chip = gtk::FlowBoxChild::new();
            chip.set_child(Some(&btn));
            {
                let pending = self.pending_tag.clone();
                let menu = self.widgets.tag_menu.clone();
                let chip_widget = chip.clone();
                let tag_id = tag.id;
                let gesture = gtk::GestureClick::new();
                gesture.set_button(3);
                gesture.connect_pressed(move |_g, _n, _x, _y| {
                    pending.set(tag_id.0);
                    // Unparent first: without this the second right-click
                    // trips `gtk_widget_set_parent` (already has a parent).
                    menu.unparent();
                    menu.set_parent(&chip_widget);
                    menu.present();
                });
                chip.add_controller(gesture);
            }
            self.widgets.tag_flow.append(&chip);
        }
        drop(ids);
    }

    pub(super) fn rebuild_notes_list(&self) {
        let list = if matches!(self.mode, ViewMode::Trash) {
            &self.trashed
        } else {
            &self.notes
        };
        crate::notes::list::render_notes(
            &self.widgets.notes_list,
            &self.widgets.notes_empty,
            &self.row_ids,
            list,
            note_row,
        );

        if let Some(id) = self.current_note {
            self.select_note_row(id);
        }
        self.refresh_cut_dim();
    }

    pub(super) fn render_search_results(&self, hits: &[SearchHit]) {
        crate::notes::list::render_search(
            &self.widgets.notes_list,
            &self.widgets.notes_empty,
            &self.row_ids,
            hits,
            note_row,
        );
    }

    pub(super) fn select_note_row(&self, id: NoteId) {
        self.suppress_selection.set(true);
        if let Some(idx) = self.row_ids.borrow().iter().position(|&x| x == id.0)
            && let Some(row) = self.widgets.notes_list.row_at_index(idx as i32)
        {
            self.widgets.notes_list.select_row(Some(&row));
        }
        self.suppress_selection.set(false);
    }

    pub(super) fn rebuild_tag_editor(&self) {
        crate::notes::tag_editor::render(
            &self.widgets.tag_editor_flow,
            &self.note_tags,
            self.current_note.map(|id| id.0),
            &self.ui_sender,
        );
    }
}
