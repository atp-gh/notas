//! Sidebar widgets: search, built-in views, and tags. The notebooks
//! section lives in [`crate::notes::notebook`] and is composed into the
//! sidebar here.

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
/// The tree model field keeps the (deprecated) `TreeStore` type, which the
/// sidebar exposes to the app coordinator for repopulation.
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

    let notebook_pane = crate::notes::notebook::build_tree_pane(window, sender, pending_notebook);

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
    root.append(&notebook_pane.header);
    root.append(&notebook_pane.scroll);
    root.append(&tags_label);
    root.append(&tag_flow);

    Sidebar {
        root,
        search_entry,
        notebook_store: notebook_pane.store,
        tag_flow,
        tag_menu,
    }
}
