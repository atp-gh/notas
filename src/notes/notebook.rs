//! GTK notebook-tree widgets: the sidebar pane that hosts the notebook
//! tree with its right-click context menu, plus hierarchy rendering.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita as adw;
use relm4::RelmWidgetExt;

use crate::notes::Clipboard;
use crate::tr;
use crate::ui::dialogs;
use crate::ui::icons;
use crate::ui::protocol::AppMsg;
use notas::core::model::{Notebook, NotebookId};

/// The widgets of the notebooks section of the sidebar. The app keeps the
/// tree store so it can repopulate rows; the header and scroll view are
/// packed into the sidebar by its caller.
#[expect(
    deprecated,
    reason = "the existing TreeView backend remains until the ColumnView migration"
)]
pub(crate) struct NotebookPane {
    /// "Notebooks" heading with the new-notebook button.
    pub header: gtk::Box,
    /// Scrolled notebook tree.
    pub scroll: gtk::ScrolledWindow,
    /// Notebook tree model, rebuilt by [`rebuild_tree`].
    pub store: gtk::TreeStore,
}

/// Build the notebooks section: heading + "new notebook" button, the
/// notebook tree, and its copy/cut/paste/rename/trash context menu.
///
/// Right-click selects the row first (so paste targets the row under the
/// cursor), anchors the popover at the cursor via `set_pointing_to`, and
/// disables Paste while the in-app clipboard is empty. Right-clicking the
/// empty area shows only Paste (target = top level).
#[expect(
    deprecated,
    reason = "the existing TreeView backend remains until the ColumnView migration"
)]
pub(crate) fn build_tree_pane(
    window: &adw::ApplicationWindow,
    sender: &relm4::Sender<AppMsg>,
    pending: &Rc<Cell<i64>>,
    clipboard: &Rc<RefCell<Option<Clipboard>>>,
) -> NotebookPane {
    let notebooks_label = gtk::Label::new(Some(tr!("Notebooks")));
    notebooks_label.set_halign(gtk::Align::Start);
    notebooks_label.set_margin_top(10);
    notebooks_label.set_margin_bottom(4);
    notebooks_label.add_css_class("heading");

    let new_notebook_button = gtk::Button::from_icon_name(icons::NEW_NOTEBOOK);
    new_notebook_button
        .set_tooltip_text(Some(tr!("New notebook (in the selected notebook, if any)")));
    new_notebook_button.set_halign(gtk::Align::End);
    new_notebook_button.set_valign(gtk::Align::Center);
    {
        let sender = sender.clone();
        new_notebook_button.connect_clicked(move |_| {
            // The app resolves the parent from the current selection:
            // a selected notebook yields a nested child, otherwise a
            // top-level notebook.
            let _ = sender.send(AppMsg::PromptNewNotebook);
        });
    }

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header.append(&notebooks_label);
    header.append(&new_notebook_button);

    let store = gtk::TreeStore::new(&[glib::Type::I64, glib::Type::STRING]);
    let notebook_tree = gtk::TreeView::with_model(&store);
    notebook_tree.set_headers_visible(false);
    notebook_tree.set_activate_on_single_click(true);
    notebook_tree.set_hexpand(true);
    {
        let column = gtk::TreeViewColumn::new();
        let renderer = gtk::CellRendererText::new();
        column.pack_start(&renderer, true);
        column.add_attribute(&renderer, "text", 1);
        notebook_tree.append_column(&column);
    }
    {
        let sender = sender.clone();
        notebook_tree.connect_row_activated(move |tree, path, _column| {
            if let Some(model) = tree.model()
                && let Some(iter) = model.iter(path)
            {
                let id: i64 = model.get_value(&iter, 0).get().unwrap_or(0);
                let _ = sender.send(AppMsg::SelectNotebook(NotebookId(id)));
            }
        });
    }

    let menu = gtk::Popover::new();
    let new_note_button = gtk::Button::with_label(tr!("New note"));
    new_note_button.set_halign(gtk::Align::Fill);
    let new_notebook_button = gtk::Button::with_label(tr!("New sub-notebook…"));
    new_notebook_button.set_halign(gtk::Align::Fill);
    let copy_button = gtk::Button::with_label(tr!("Copy"));
    copy_button.set_halign(gtk::Align::Fill);
    let cut_button = gtk::Button::with_label(tr!("Cut"));
    cut_button.set_halign(gtk::Align::Fill);
    let paste_button = gtk::Button::with_label(tr!("Paste"));
    paste_button.set_halign(gtk::Align::Fill);
    let rename_button = gtk::Button::with_label(tr!("Rename…"));
    rename_button.set_halign(gtk::Align::Fill);
    let trash_button = gtk::Button::with_label(tr!("Move to trash"));
    trash_button.set_halign(gtk::Align::Fill);
    trash_button.add_css_class("destructive-action");
    {
        let sender = sender.clone();
        let pending = pending.clone();
        let menu = menu.clone();
        new_note_button.connect_clicked(move |_| {
            // `pending` holds -1 on the empty area (see the gesture
            // below), which maps to the notebook root / unfiled.
            let target = {
                let id = pending.get();
                (id >= 0).then_some(NotebookId(id))
            };
            let _ = sender.send(AppMsg::NewNoteAt(target));
            menu.popdown();
        });
    }
    {
        let window = window.clone();
        let pending = pending.clone();
        let sender = sender.clone();
        new_notebook_button.connect_clicked(move |_| {
            let parent = {
                let id = pending.get();
                (id >= 0).then_some(NotebookId(id))
            };
            let title = if parent.is_some() {
                tr!("New sub-notebook")
            } else {
                tr!("New notebook")
            };
            dialogs::input(
                &window,
                &sender,
                title,
                tr!("Notebook name…"),
                "",
                move |name| AppMsg::NewNotebook { parent, name },
            );
        });
    }
    {
        let sender = sender.clone();
        let pending = pending.clone();
        let menu = menu.clone();
        copy_button.connect_clicked(move |_| {
            let _ = sender.send(AppMsg::CopyNotebook(NotebookId(pending.get())));
            menu.popdown();
        });
    }
    {
        let sender = sender.clone();
        let pending = pending.clone();
        let menu = menu.clone();
        cut_button.connect_clicked(move |_| {
            let _ = sender.send(AppMsg::CutNotebook(NotebookId(pending.get())));
            menu.popdown();
        });
    }
    {
        let sender = sender.clone();
        let pending = pending.clone();
        let menu = menu.clone();
        paste_button.connect_clicked(move |_| {
            // Paste lands in the row under the cursor; when the menu was
            // opened on empty space `pending` holds -1 (see the gesture
            // below) and the paste targets the top level.
            let target = {
                let id = pending.get();
                (id >= 0).then_some(NotebookId(id))
            };
            let _ = sender.send(AppMsg::PasteToNotebook(target));
            menu.popdown();
        });
    }
    {
        let window = window.clone();
        let pending = pending.clone();
        let sender = sender.clone();
        rename_button.connect_clicked(move |_| {
            let id = pending.get();
            dialogs::input(
                &window,
                &sender,
                tr!("Rename notebook"),
                tr!("Notebook name…"),
                "",
                move |name| AppMsg::RenameNotebook {
                    id: NotebookId(id),
                    name,
                },
            );
        });
    }
    {
        let sender = sender.clone();
        let pending = pending.clone();
        let menu = menu.clone();
        trash_button.connect_clicked(move |_| {
            let _ = sender.send(AppMsg::TrashNotebook(NotebookId(pending.get())));
            menu.popdown();
        });
    }
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    menu_box.set_margin_all(8);
    menu_box.append(&new_note_button);
    menu_box.append(&new_notebook_button);
    menu_box.append(&copy_button);
    menu_box.append(&cut_button);
    menu_box.append(&paste_button);
    menu_box.append(&rename_button);
    menu_box.append(&trash_button);
    menu.set_child(Some(&menu_box));

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&notebook_tree));
    scroll.set_vexpand(true);
    // Parent once to the (stable) scrolled window, never to the tree:
    // `set_parent` onto the TreeView trips `gtk_css_node_insert_after`
    // criticals (seen at startup), and re-parenting per click trips
    // `gtk_widget_set_parent` (the popover already has a parent). The cursor
    // position still comes from `set_pointing_to` per click, translated
    // into the scrolled window's coordinates below.
    menu.set_parent(&scroll);
    {
        let tree = notebook_tree.clone();
        let menu = menu;
        let pending = pending.clone();
        let clipboard = clipboard.clone();
        let scrolled = scroll.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        gesture.connect_pressed(move |gesture, _n, x, y| {
            // The gesture reports tree coordinates but the popover is
            // parented to the scrolled window, so translate (this also
            // accounts for the current scroll offset).
            let point = gtk::graphene::Point::new(x as f32, y as f32);
            let Some(anchor) = tree.compute_point(&scrolled, &point) else {
                return;
            };
            let rect = gtk::gdk::Rectangle::new(anchor.x() as i32, anchor.y() as i32, 1, 1);
            menu.set_pointing_to(Some(&rect));
            if let Some((path, _column, _x, _y)) = tree.path_at_pos(x as i32, y as i32)
                && let Some(path) = path
                && let Some(model) = tree.model()
                && let Some(iter) = model.iter(&path)
            {
                let id: i64 = model.get_value(&iter, 0).get().unwrap_or(0);
                pending.set(id);
                // Row menu: new note / sub-notebook target the row under
                // the cursor; paste needs a buffered entry.
                new_note_button.set_visible(true);
                new_notebook_button.set_visible(true);
                new_notebook_button.set_label(tr!("New sub-notebook…"));
                for btn in [&copy_button, &cut_button, &rename_button, &trash_button] {
                    btn.set_visible(true);
                }
                paste_button.set_visible(true);
                paste_button.set_sensitive(clipboard.borrow().is_some());
                menu.popup();
            } else {
                // Empty-area menu: creation targets the notebook root
                // (unfiled notes / top-level notebooks); paste targets
                // the top level as well.
                pending.set(-1);
                new_note_button.set_visible(true);
                new_notebook_button.set_visible(true);
                new_notebook_button.set_label(tr!("New notebook…"));
                for btn in [&copy_button, &cut_button, &rename_button, &trash_button] {
                    btn.set_visible(false);
                }
                paste_button.set_visible(true);
                paste_button.set_sensitive(clipboard.borrow().is_some());
                menu.popup();
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        notebook_tree.add_controller(gesture);
    }

    NotebookPane {
        header,
        scroll,
        store,
    }
}

/// Order notebooks so every parent precedes its children, no matter how
/// the input is sorted. The sidebar's list comes from
/// [`notas::core::repository::Repository::list_notebooks`], which orders
/// by name — a child like `AI` under `Interview` can therefore precede its
/// parent — and GTK requires a parent row to exist before its children
/// are inserted. The result keeps the input's relative order within each
/// level. A notebook whose parent id is missing (corrupt data) is placed
/// at the root rather than dropped or looped over forever.
pub(crate) fn parent_first_order(notebooks: &[Notebook]) -> Vec<&Notebook> {
    let mut ordered: Vec<&Notebook> = Vec::with_capacity(notebooks.len());
    let mut pending: Vec<&Notebook> = notebooks.iter().collect();
    while !pending.is_empty() {
        let mut next_pending = Vec::new();
        let mut progressed = false;
        for notebook in pending {
            let parent_ready = notebook
                .parent_id
                .is_none_or(|parent_id| ordered.iter().any(|n| n.id == parent_id));
            if parent_ready {
                ordered.push(notebook);
                progressed = true;
            } else {
                next_pending.push(notebook);
            }
        }
        if !progressed {
            // Only orphans remain; hang them at the root instead of looping.
            ordered.extend(next_pending);
            break;
        }
        pending = next_pending;
    }
    ordered
}

/// Rebuild a notebook tree from the core model list.
#[expect(
    deprecated,
    reason = "the existing TreeView backend remains until the ColumnView migration"
)]
pub(crate) fn rebuild_tree(store: &gtk::TreeStore, notebooks: &[Notebook]) {
    store.clear();
    let mut iters = std::collections::HashMap::new();
    for notebook in parent_first_order(notebooks) {
        let parent = notebook
            .parent_id
            .and_then(|parent_id| iters.get(&parent_id).cloned());
        let iter = store.insert_with_values(
            parent.as_ref(),
            None,
            &[(0, &notebook.id.0), (1, &notebook.name)],
        );
        iters.insert(notebook.id, iter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nb(id: i64, parent_id: Option<i64>) -> Notebook {
        Notebook {
            id: NotebookId(id),
            parent_id: parent_id.map(NotebookId),
            name: format!("nb{id}"),
            is_trashed: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn child_sorted_before_parent_is_reordered_after_it() {
        // The name-sorted sidebar order would put the child "AI" before its
        // parent "Interview"; the orderer must fix exactly that.
        let notebooks = vec![nb(2, Some(1)), nb(1, None)];
        let ordered = parent_first_order(&notebooks);
        let ids: Vec<i64> = ordered.iter().map(|n| n.id.0).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn deep_chain_is_built_root_first() {
        let notebooks = vec![nb(3, Some(2)), nb(1, None), nb(2, Some(1))];
        let ids: Vec<i64> = parent_first_order(&notebooks)
            .iter()
            .map(|n| n.id.0)
            .collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn siblings_keep_input_order() {
        let notebooks = vec![nb(2, None), nb(1, None)];
        let ids: Vec<i64> = parent_first_order(&notebooks)
            .iter()
            .map(|n| n.id.0)
            .collect();
        assert_eq!(ids, vec![2, 1]);
    }

    #[test]
    fn orphaned_notebook_is_placed_at_root() {
        let notebooks = vec![nb(2, Some(99)), nb(1, None)];
        let ids: Vec<i64> = parent_first_order(&notebooks)
            .iter()
            .map(|n| n.id.0)
            .collect();
        assert_eq!(ids, vec![1, 2]);
    }
}
