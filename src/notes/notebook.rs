//! GTK notebook-tree widgets: the sidebar pane that hosts the notebook
//! tree with its right-click context menu, plus hierarchy rendering.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita as adw;
use relm4::RelmWidgetExt;

use crate::tr;
use crate::ui::dialogs;
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
/// notebook tree, and its rename/delete context menu (opened on a
/// right-click, remembered via `pending`).
#[expect(
    deprecated,
    reason = "the existing TreeView backend remains until the ColumnView migration"
)]
pub(crate) fn build_tree_pane(
    window: &adw::ApplicationWindow,
    sender: &relm4::Sender<AppMsg>,
    pending: &Rc<Cell<i64>>,
) -> NotebookPane {
    let notebooks_label = gtk::Label::new(Some(tr!("Notebooks")));
    notebooks_label.set_halign(gtk::Align::Start);
    notebooks_label.set_margin_top(10);
    notebooks_label.set_margin_bottom(4);
    notebooks_label.add_css_class("heading");

    let new_notebook_button = gtk::Button::from_icon_name("folder-new-symbolic");
    new_notebook_button.set_tooltip_text(Some(tr!("New notebook")));
    new_notebook_button.set_halign(gtk::Align::End);
    new_notebook_button.set_valign(gtk::Align::Center);
    {
        let window = window.clone();
        let sender = sender.clone();
        new_notebook_button.connect_clicked(move |_| {
            dialogs::input(
                &window,
                &sender,
                tr!("New notebook"),
                tr!("Notebook name…"),
                "",
                AppMsg::NewNotebook,
            );
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
    let rename_button = gtk::Button::with_label(tr!("Rename…"));
    rename_button.set_halign(gtk::Align::Fill);
    let delete_button = gtk::Button::with_label(tr!("Delete notebook"));
    delete_button.set_halign(gtk::Align::Fill);
    delete_button.add_css_class("destructive-action");
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
        delete_button.connect_clicked(move |_| {
            let _ = sender.send(AppMsg::DeleteNotebook(NotebookId(pending.get())));
        });
    }
    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    menu_box.set_margin_all(8);
    menu_box.append(&rename_button);
    menu_box.append(&delete_button);
    menu.set_child(Some(&menu_box));
    {
        let tree = notebook_tree.clone();
        let menu = menu;
        let pending = pending.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        gesture.connect_pressed(move |_gesture, _n, x, y| {
            if let Some((path, _column, _x, _y)) = tree.path_at_pos(x as i32, y as i32)
                && let Some(path) = path
                && let Some(model) = tree.model()
                && let Some(iter) = model.iter(&path)
            {
                let id: i64 = model.get_value(&iter, 0).get().unwrap_or(0);
                pending.set(id);
                menu.set_parent(&tree);
                menu.present();
            }
        });
        notebook_tree.add_controller(gesture);
    }

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&notebook_tree));
    scroll.set_vexpand(true);

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
