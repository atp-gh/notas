//! Sidebar widgets: search, built-in views, notebooks, and tags.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita as adw;
use relm4::RelmWidgetExt;

use crate::application::ViewId;
use crate::tr;
use crate::ui::dialogs;
use crate::ui::protocol::AppMsg;

/// GTK handles owned by the sidebar and touched while the app is running.
#[expect(
    deprecated,
    reason = "TreeView migration to ColumnView is tracked separately"
)]
pub(crate) struct Sidebar {
    /// Root sidebar container.
    pub root: gtk::Box,
    /// Search field.
    pub search_entry: gtk::SearchEntry,
    /// Notebook tree model.
    pub notebook_store: gtk::TreeStore,
    /// Tag chip container.
    pub tag_flow: gtk::FlowBox,
    /// Notebook/tag context menu for the app coordinator to parent.
    pub tag_menu: gtk::Popover,
}

/// Build the complete sidebar and wire semantic messages to the coordinator.
#[expect(
    deprecated,
    reason = "TreeView migration to ColumnView is tracked separately"
)]
pub(crate) fn build(
    window: &adw::ApplicationWindow,
    sender: &relm4::Sender<AppMsg>,
    pending_notebook: &Rc<Cell<i64>>,
    pending_tag: &Rc<Cell<i64>>,
) -> Sidebar {
    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some(tr!("Search notes…")));
    search_entry.set_margin_bottom(6);
    {
        let sender = sender.clone();
        search_entry.connect_search_changed(move |entry| {
            let _ = sender.send(AppMsg::SearchChanged(entry.text().to_string()));
        });
    }

    let view_list = gtk::ListBox::new();
    view_list.set_selection_mode(gtk::SelectionMode::Single);
    view_list.set_activate_on_single_click(true);
    for (label, id) in [
        (tr!("All notes"), ViewId::All),
        (tr!("Unfiled"), ViewId::Unfiled),
        (tr!("Trash"), ViewId::Trash),
    ] {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(match id {
            ViewId::All => "all",
            ViewId::Unfiled => "unfiled",
            ViewId::Trash => "trash",
        });
        row.set_child(Some(&gtk::Label::new(Some(label))));
        row.set_activatable(true);
        view_list.append(&row);
    }
    {
        let sender = sender.clone();
        view_list.connect_row_selected(move |_list, row| {
            if let Some(row) = row {
                let view = match row.widget_name().as_str() {
                    "trash" => ViewId::Trash,
                    "unfiled" => ViewId::Unfiled,
                    _ => ViewId::All,
                };
                let _ = sender.send(AppMsg::SelectView(view));
            }
        });
    }

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

    let notebooks_header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    notebooks_header.append(&notebooks_label);
    notebooks_header.append(&new_notebook_button);

    let notebook_store = gtk::TreeStore::new(&[glib::Type::I64, glib::Type::STRING]);
    let notebook_tree = gtk::TreeView::with_model(&notebook_store);
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
                let _ = sender.send(AppMsg::SelectNotebook(crate::core::model::NotebookId(id)));
            }
        });
    }

    let tree_menu = gtk::Popover::new();
    let rename_notebook_button = gtk::Button::with_label(tr!("Rename…"));
    rename_notebook_button.set_halign(gtk::Align::Fill);
    let delete_notebook_button = gtk::Button::with_label(tr!("Delete notebook"));
    delete_notebook_button.set_halign(gtk::Align::Fill);
    delete_notebook_button.add_css_class("destructive-action");
    {
        let window = window.clone();
        let pending = pending_notebook.clone();
        let sender = sender.clone();
        rename_notebook_button.connect_clicked(move |_| {
            let id = pending.get();
            dialogs::input(
                &window,
                &sender,
                tr!("Rename notebook"),
                tr!("Notebook name…"),
                "",
                move |name| AppMsg::RenameNotebook {
                    id: crate::core::model::NotebookId(id),
                    name,
                },
            );
        });
    }
    {
        let sender = sender.clone();
        let pending = pending_notebook.clone();
        delete_notebook_button.connect_clicked(move |_| {
            let _ = sender.send(AppMsg::DeleteNotebook(crate::core::model::NotebookId(
                pending.get(),
            )));
        });
    }
    let tree_menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    tree_menu_box.set_margin_all(8);
    tree_menu_box.append(&rename_notebook_button);
    tree_menu_box.append(&delete_notebook_button);
    tree_menu.set_child(Some(&tree_menu_box));
    {
        let tree = notebook_tree.clone();
        let menu = tree_menu;
        let pending = pending_notebook.clone();
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

    let tree_scroll = gtk::ScrolledWindow::new();
    tree_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    tree_scroll.set_child(Some(&notebook_tree));
    tree_scroll.set_vexpand(true);

    let tags_label = gtk::Label::new(Some(tr!("Tags")));
    tags_label.set_halign(gtk::Align::Start);
    tags_label.set_margin_top(10);
    tags_label.set_margin_bottom(4);
    tags_label.add_css_class("heading");

    let tag_flow = gtk::FlowBox::new();
    tag_flow.set_selection_mode(gtk::SelectionMode::None);
    tag_flow.set_min_children_per_line(1);

    let tag_menu = gtk::Popover::new();
    let rename_tag_button = gtk::Button::with_label(tr!("Rename…"));
    rename_tag_button.set_halign(gtk::Align::Fill);
    let delete_tag_button = gtk::Button::with_label(tr!("Delete tag"));
    delete_tag_button.set_halign(gtk::Align::Fill);
    delete_tag_button.add_css_class("destructive-action");
    {
        let window = window.clone();
        let pending = pending_tag.clone();
        let sender = sender.clone();
        rename_tag_button.connect_clicked(move |_| {
            let id = pending.get();
            dialogs::input(
                &window,
                &sender,
                tr!("Rename tag"),
                tr!("Tag name…"),
                "",
                move |name| AppMsg::RenameTag {
                    id: crate::core::model::TagId(id),
                    name,
                },
            );
        });
    }
    {
        let sender = sender.clone();
        let pending = pending_tag.clone();
        delete_tag_button.connect_clicked(move |_| {
            let _ = sender.send(AppMsg::DeleteTag(crate::core::model::TagId(pending.get())));
        });
    }
    let tag_menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    tag_menu_box.set_margin_all(8);
    tag_menu_box.append(&rename_tag_button);
    tag_menu_box.append(&delete_tag_button);
    tag_menu.set_child(Some(&tag_menu_box));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_width_request(230);
    root.set_margin_all(8);
    root.append(&search_entry);
    root.append(&view_list);
    root.append(&notebooks_header);
    root.append(&tree_scroll);
    root.append(&tags_label);
    root.append(&tag_flow);

    Sidebar {
        root,
        search_entry,
        notebook_store,
        tag_flow,
        tag_menu,
    }
}
