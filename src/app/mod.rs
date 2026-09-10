//! Main application component: three-pane layout (sidebar | note list |
//! editor), wires the DB worker to the widgets, and owns all app state.
//!
//! The widget tree is built imperatively in `init` and stored inside the
//! model so that message handlers can touch widgets directly. The initial
//! pane geometry and its display probes live in [`layout`]; this module
//! keeps the component, its message handling and its state.

pub mod db_worker;

mod actions;
mod layout;
mod messages;
use layout::wire_initial_split;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita::prelude::*;
use relm4::prelude::*;
use relm4::{ComponentParts, ComponentSender, Controller, SimpleComponent};
use sourceview5::prelude::*;
use sqlx::SqlitePool;

use notas::core::model::{Note, NoteId, Notebook, Tag, TagCount, TagId};

use crate::app::db_worker::DbWorker;
use crate::application::DbMsg;
use crate::application::config::Settings;
use crate::editor::{Editor, build_editor};
use crate::notes::Clipboard;
use crate::tr;
use crate::ui::settings::build_settings_window;
use crate::ui::status::sync_indicator_text;
use crate::ui::theme;
pub use crate::ui::{AppMsg, EditorMode, ViewMode};

type AppSender = relm4::Sender<AppMsg>;

pub struct App {
    widgets: Widgets,
    worker: Controller<DbWorker>,
    /// Sender used by widget closures (tag chips etc.) created after `init`.
    ui_sender: AppSender,
    settings: Settings,
    mode: ViewMode,
    notebooks: Vec<Notebook>,
    notes: Vec<Note>,
    trashed: Vec<Note>,
    tags: Vec<TagCount>,
    note_tags: Vec<Tag>,
    current_note: Option<NoteId>,
    saved_title: String,
    saved_content: String,
    dirty: bool,
    editor_mode: EditorMode,
    active_tag: Option<TagId>,
    selected_trashed: Option<NoteId>,
    pending_open: Option<NoteId>,
    pending_new_note: bool,
    pending_close: bool,
    /// Export directory waiting for the user to confirm the import.
    pending_import: Option<PathBuf>,
    row_ids: Rc<RefCell<Vec<i64>>>,
    tag_ids: Rc<RefCell<Vec<i64>>>,
    loading: Rc<Cell<bool>>,
    allow_close: Rc<Cell<bool>>,
    suppress_selection: Rc<Cell<bool>>,
    suppress_tag_toggle: Rc<Cell<bool>>,
    pending_tag: Rc<Cell<i64>>,
    /// In-app copy/cut buffer for right-click paste (shared with the menu
    /// gesture closures so Paste can grey out while empty).
    clipboard: Rc<RefCell<Option<Clipboard>>>,
}

/// Widget handles the message handlers touch. Widgets that are created and
/// wired up in `init` but never touched again (sidebar list, tag entry)
/// intentionally stay local to `init`; GTK keeps them alive through the
/// parent/child references.
#[expect(
    deprecated,
    reason = "notebook sidebar uses gtk::TreeView; migrating to gtk::ColumnView is a separate UI task"
)]
pub struct Widgets {
    window: adw::ApplicationWindow,
    dirty_label: gtk::Label,
    status_label: gtk::Label,
    /// Always-on sync state (last successful sync, or “Syncing…”).
    sync_label: gtk::Label,
    save_btn: gtk::Button,
    // sidebar
    search_entry: gtk::SearchEntry,
    notebook_store: gtk::TreeStore,
    tag_flow: gtk::FlowBox,
    tag_menu: gtk::Popover,
    // middle
    view_title: gtk::Label,
    notes_list: gtk::ListBox,
    notes_empty: gtk::Label,
    restore_btn: gtk::Button,
    delete_btn: gtk::Button,
    // The right-click menu boxes (kept so `update_trash_buttons` can swap
    // the live/trash sets on mode change; the popover itself lives in the
    // gesture closures that own a clone each).
    note_normal_box: gtk::Box,
    note_trash_box: gtk::Box,
    // editor
    title_entry: gtk::Entry,
    editor: Editor,
    mode_source_btn: gtk::ToggleButton,
    mode_split_btn: gtk::ToggleButton,
    mode_preview_btn: gtk::ToggleButton,
    tag_editor_flow: gtk::FlowBox,
    // bottom bar / settings
    status_bar: gtk::Box,
    settings_window: adw::PreferencesDialog,
}

/// Everything the app needs to start: the DB pool plus persisted settings.
pub struct AppInit {
    pub pool: SqlitePool,
    pub settings: Settings,
}

impl SimpleComponent for App {
    type Init = AppInit;
    type Input = AppMsg;
    type Output = ();
    type Widgets = ();
    type Root = adw::ApplicationWindow;

    fn init_root() -> Self::Root {
        adw::ApplicationWindow::builder()
            .title("Notas")
            .default_width(1180)
            .default_height(760)
            .build()
    }

    fn init(
        init: Self::Init,
        window: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let AppInit { pool, settings } = init;
        let app_sender = sender.input_sender().clone();

        // Apply the persisted color scheme before anything reads the style
        // manager (the editor picks up its initial GtkSourceView scheme and
        // preview palette from `is_dark()` at build time).
        theme::apply(settings.theme.mode);

        let loading = Rc::new(Cell::new(false));
        let allow_close = Rc::new(Cell::new(false));
        let suppress_selection = Rc::new(Cell::new(false));
        let suppress_tag_toggle = Rc::new(Cell::new(false));
        let row_ids: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let tag_ids: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let pending_nb = Rc::new(Cell::new(0));
        let pending_tag = Rc::new(Cell::new(0));
        let pending_note = Rc::new(Cell::new(0));
        let clipboard: Rc<RefCell<Option<Clipboard>>> = Rc::new(RefCell::new(None));

        let worker: Controller<DbWorker> = relm4::ComponentBuilder::<DbWorker>::default()
            .launch(pool)
            .forward(&app_sender, AppMsg::Db);

        let emit = {
            let s = app_sender.clone();
            move |msg| {
                let _ = s.send(msg);
            }
        };

        // ------------------------------------------------------------- sidebar
        let sidebar_parts = crate::notes::sidebar::build(
            &window,
            &app_sender,
            &pending_nb,
            &pending_tag,
            &clipboard,
        );
        let sidebar = sidebar_parts.root;
        let search_entry = sidebar_parts.search_entry;
        let notebook_store = sidebar_parts.notebook_store;
        let tag_flow = sidebar_parts.tag_flow;
        let tag_menu = sidebar_parts.tag_menu;

        // ------------------------------------------------------------- middle
        let view_title = gtk::Label::new(Some(tr!("All notes")));
        view_title.set_halign(gtk::Align::Start);
        view_title.add_css_class("title-2");

        let new_note_btn = gtk::Button::from_icon_name("document-new-symbolic");
        new_note_btn.set_tooltip_text(Some(tr!("New note (Ctrl+N)")));
        new_note_btn.set_valign(gtk::Align::Center);
        {
            let emit = emit.clone();
            new_note_btn.connect_clicked(move |_| emit(AppMsg::NewNote));
        }

        let restore_btn = gtk::Button::with_label(tr!("Restore"));
        restore_btn.set_visible(false);
        let delete_btn = gtk::Button::with_label(tr!("Delete forever"));
        delete_btn.set_visible(false);
        delete_btn.add_css_class("destructive-action");
        {
            let s = emit.clone();
            restore_btn.connect_clicked(move |_| s(AppMsg::RestoreNote));
            let s = emit.clone();
            delete_btn.connect_clicked(move |_| s(AppMsg::DeleteForever));
        }

        let middle_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        middle_header.append(&view_title);
        middle_header.append(&new_note_btn);
        middle_header.set_hexpand(true);
        middle_header.append(&restore_btn);
        middle_header.append(&delete_btn);

        let notes_list = gtk::ListBox::new();
        notes_list.set_selection_mode(gtk::SelectionMode::Single);
        notes_list.set_activate_on_single_click(true);
        {
            let emit = emit.clone();
            let suppress = suppress_selection.clone();
            let row_ids = row_ids.clone();
            notes_list.connect_row_selected(move |_list, row| {
                if suppress.get() {
                    return;
                }
                if let Some(row) = row {
                    let idx = row.index() as usize;
                    if let Some(&id) = row_ids.borrow().get(idx) {
                        emit(AppMsg::SelectNote(NoteId(id)));
                    }
                }
            });
        }

        let notes_empty = gtk::Label::new(Some(tr!("No notes yet")));
        notes_empty.add_css_class("dim-label");
        notes_empty.set_margin_top(24);

        // Note right-click menu: copy/cut/paste/trash in normal views,
        // restore/delete-forever in the trash view (boxes toggled by
        // `update_trash_buttons`). The gesture selects the row first so
        // paste targets the note under the cursor; paste needs a buffered
        // entry and greys out while the clipboard is empty.
        let note_menu = gtk::Popover::new();
        let note_normal_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        note_normal_box.set_margin_all(8);
        let note_copy_btn = gtk::Button::with_label(tr!("Copy"));
        let note_cut_btn = gtk::Button::with_label(tr!("Cut"));
        let note_paste_btn = gtk::Button::with_label(tr!("Paste"));
        let note_trash_btn = gtk::Button::with_label(tr!("Move to trash"));
        note_trash_btn.add_css_class("destructive-action");
        note_normal_box.append(&note_copy_btn);
        note_normal_box.append(&note_cut_btn);
        note_normal_box.append(&note_paste_btn);
        note_normal_box.append(&note_trash_btn);
        let note_trash_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        note_trash_box.set_margin_all(8);
        note_trash_box.set_visible(false);
        let note_restore_btn = gtk::Button::with_label(tr!("Restore"));
        let note_delete_btn = gtk::Button::with_label(tr!("Delete forever"));
        note_delete_btn.add_css_class("destructive-action");
        note_trash_box.append(&note_restore_btn);
        note_trash_box.append(&note_delete_btn);
        let note_menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        note_menu_box.append(&note_normal_box);
        note_menu_box.append(&note_trash_box);
        note_menu.set_child(Some(&note_menu_box));
        // Parent once to the (stable) list: parenting to the clicked row
        // destroys the popover with the row on the next list rebuild
        // ("Finalizing GtkListBoxRow, but it still has children"), and
        // re-parenting per click trips `gtk_widget_set_parent`. The cursor
        // position still comes from `set_pointing_to` per click.
        note_menu.set_parent(&notes_list);
        {
            let s = app_sender.clone();
            let pending = pending_note.clone();
            let menu = note_menu.clone();
            note_copy_btn.connect_clicked(move |_| {
                let _ = s.send(AppMsg::CopyNote(NoteId(pending.get())));
                menu.popdown();
            });
            let s = app_sender.clone();
            let pending = pending_note.clone();
            let menu = note_menu.clone();
            note_cut_btn.connect_clicked(move |_| {
                let _ = s.send(AppMsg::CutNote(NoteId(pending.get())));
                menu.popdown();
            });
            let s = app_sender.clone();
            let pending = pending_note.clone();
            let menu = note_menu.clone();
            note_paste_btn.connect_clicked(move |_| {
                let _ = s.send(AppMsg::PasteToNote(NoteId(pending.get())));
                menu.popdown();
            });
            let s = app_sender.clone();
            let pending = pending_note.clone();
            let menu = note_menu.clone();
            note_trash_btn.connect_clicked(move |_| {
                let _ = s.send(AppMsg::TrashNoteById(NoteId(pending.get())));
                menu.popdown();
            });
            let s = app_sender.clone();
            let menu = note_menu.clone();
            note_restore_btn.connect_clicked(move |_| {
                let _ = s.send(AppMsg::RestoreNote);
                menu.popdown();
            });
            let s = app_sender.clone();
            let menu = note_menu.clone();
            note_delete_btn.connect_clicked(move |_| {
                let _ = s.send(AppMsg::DeleteForever);
                menu.popdown();
            });
        }
        {
            let list = notes_list.clone();
            let ids = row_ids.clone();
            let board = clipboard.clone();
            let suppress = suppress_selection.clone();
            let s = app_sender.clone();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(3);
            gesture.connect_pressed(move |gesture, _n, x, y| {
                let Some(row) = list.row_at_y(y as i32) else {
                    return;
                };
                let idx = row.index() as usize;
                let Some(&id) = ids.borrow().get(idx) else {
                    return;
                };
                pending_note.set(id);
                // Highlight without loading: the menu actions carry the
                // id explicitly, except in the trash view where
                // restore/delete act on the list selection.
                suppress.set(true);
                list.select_row(Some(&row));
                suppress.set(false);
                let _ = s.send(AppMsg::SelectNote(NoteId(id)));
                note_paste_btn.set_sensitive(board.borrow().is_some());
                let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
                note_menu.set_pointing_to(Some(&rect));
                note_menu.popup();
                gesture.set_state(gtk::EventSequenceState::Claimed);
            });
            notes_list.add_controller(gesture);
        }

        let notes_scroll = gtk::ScrolledWindow::new();
        notes_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        notes_scroll.set_child(Some(&notes_list));
        notes_scroll.set_vexpand(true);

        let middle = gtk::Box::new(gtk::Orientation::Vertical, 6);
        middle.set_width_request(300);
        middle.set_margin_all(8);
        middle.append(&middle_header);
        middle.append(&notes_scroll);
        middle.append(&notes_empty);

        // ------------------------------------------------------------- editor
        let editor = build_editor(emit.clone(), loading.clone());
        editor
            .source_view
            .set_show_line_numbers(settings.editor.show_line_numbers);

        let title_entry = gtk::Entry::new();
        title_entry.set_placeholder_text(Some(tr!("Title")));
        title_entry.add_css_class("title-1");
        {
            let emit = emit.clone();
            title_entry.connect_changed(move |_| emit(AppMsg::TitleChanged));
        }

        let editor_stack_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
        editor_stack_area.append(&editor.search_bar);
        editor_stack_area.append(&editor.split);

        // Note tags: an entry to add + a flow of removable chips.
        let tag_editor_flow = gtk::FlowBox::new();
        tag_editor_flow.set_selection_mode(gtk::SelectionMode::None);
        let tag_entry = gtk::Entry::new();
        tag_entry.set_placeholder_text(Some(tr!("Add tag…")));
        tag_entry.set_width_chars(18);
        {
            let emit = emit.clone();
            let flow = tag_editor_flow.clone();
            tag_entry.connect_activate(move |entry| {
                let mut names: Vec<String> = Vec::new();
                let mut child = flow.first_child();
                while let Some(w) = child {
                    let next = w.next_sibling();
                    if let Some(btn) = w
                        .downcast::<gtk::FlowBoxChild>()
                        .ok()
                        .and_then(|c| c.child())
                        .and_then(|w| w.downcast::<gtk::Button>().ok())
                        && let Some(label) =
                            btn.child().and_then(|w| w.downcast::<gtk::Label>().ok())
                    {
                        let text = label.text().to_string();
                        if let Some(stripped) = text.strip_prefix("× ") {
                            names.push(stripped.to_string());
                        }
                    }
                    child = next;
                }
                let text = entry.text().trim().to_string();
                if !text.is_empty() && !names.contains(&text) {
                    names.push(text);
                }
                emit(AppMsg::TagsEdited(names));
                entry.set_text("");
            });
        }
        let tag_editor_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        tag_editor_row.append(&tag_entry);
        tag_editor_row.append(&tag_editor_flow);
        tag_editor_row.set_margin_top(4);
        tag_editor_row.set_margin_bottom(4);

        let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_bar.set_margin_top(4);
        let dirty_label = gtk::Label::new(Some(tr!("All changes saved")));
        dirty_label.set_halign(gtk::Align::Start);
        let status_label = gtk::Label::new(None);
        status_label.set_halign(gtk::Align::Start);
        status_label.set_hexpand(true);
        status_label.set_ellipsize(pango::EllipsizeMode::End);
        // Persistent sync indicator on the right side of the bar: the last
        // successful sync time, or “Syncing…” while a sync is in flight.
        let sync_label = gtk::Label::new(None);
        sync_label.set_halign(gtk::Align::End);
        sync_label.set_ellipsize(pango::EllipsizeMode::End);
        sync_label.set_tooltip_text(Some(tr!("Sync status — ☰ menu → Sync now…")));
        sync_label.set_text(&sync_indicator_text(&settings.sync.last_synced_at));
        let save_btn = gtk::Button::from_icon_name("document-save-symbolic");
        save_btn.set_tooltip_text(Some(tr!("Save (Ctrl+S)")));
        save_btn.set_sensitive(false);
        {
            let emit = emit.clone();
            save_btn.connect_clicked(move |_| emit(AppMsg::SaveNote));
        }
        status_bar.append(&dirty_label);
        status_bar.append(&status_label);
        status_bar.append(&sync_label);
        status_bar.append(&save_btn);
        status_bar.set_visible(settings.interface.show_status_bar);

        let editor_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        editor_pane.set_margin_all(8);
        editor_pane.append(&title_entry);
        editor_pane.append(&tag_editor_row);
        editor_pane.append(&editor_stack_area);
        editor_pane.append(&status_bar);
        editor_stack_area.set_vexpand(true);

        // ------------------------------------------------------------- header
        let trash_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        trash_btn.set_tooltip_text(Some(tr!("Move note to trash")));
        {
            let emit = emit.clone();
            trash_btn.connect_clicked(move |_| emit(AppMsg::TrashNote));
        }

        // Joplin-style tri-state switch: editor-only | live split |
        // preview-only. Three small icon buttons in a linked box; only
        // activating a button emits, so the handler just mirrors state back.
        let mode_source_btn = gtk::ToggleButton::new();
        mode_source_btn.set_icon_name("document-edit-symbolic");
        mode_source_btn.set_tooltip_text(Some(tr!("Editor only")));
        let mode_split_btn = gtk::ToggleButton::new();
        mode_split_btn.set_icon_name("view-dual-symbolic");
        mode_split_btn.set_tooltip_text(Some(tr!("Split: editor + live preview (Ctrl+E)")));
        mode_split_btn.set_active(true);
        let mode_preview_btn = gtk::ToggleButton::new();
        mode_preview_btn.set_icon_name("view-reveal-symbolic");
        mode_preview_btn.set_tooltip_text(Some(tr!("Preview only")));
        let mode_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        mode_box.add_css_class("linked");
        mode_box.append(&mode_source_btn);
        mode_box.append(&mode_split_btn);
        mode_box.append(&mode_preview_btn);
        {
            let emit_source = emit.clone();
            mode_source_btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    emit_source(AppMsg::SetEditorMode(EditorMode::Source));
                }
            });
            let emit_split = emit.clone();
            mode_split_btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    emit_split(AppMsg::SetEditorMode(EditorMode::Split));
                }
            });
            let emit_preview = emit.clone();
            mode_preview_btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    emit_preview(AppMsg::SetEditorMode(EditorMode::Preview));
                }
            });
        }

        let import_btn = gtk::Button::with_label(tr!("Import Markdown…"));
        let export_btn = gtk::Button::with_label(tr!("Export Markdown…"));
        let backup_btn = gtk::Button::with_label(tr!("Backup database…"));
        let sync_btn = gtk::Button::with_label(tr!("Sync now…"));
        let settings_btn = gtk::Button::with_label(tr!("Settings…"));
        let quit_btn = gtk::Button::with_label(tr!("Quit"));
        {
            let s = emit.clone();
            import_btn.connect_clicked(move |_| s(AppMsg::ImportMarkdown));
            let s = emit.clone();
            export_btn.connect_clicked(move |_| s(AppMsg::ExportMarkdown));
            let s = emit.clone();
            backup_btn.connect_clicked(move |_| s(AppMsg::BackupNow));
            let s = emit.clone();
            sync_btn.connect_clicked(move |_| s(AppMsg::SyncNow));
            let s = emit.clone();
            settings_btn.connect_clicked(move |_| s(AppMsg::OpenSettings));
            let s = emit.clone();
            quit_btn.connect_clicked(move |_| s(AppMsg::CloseRequested));
        }
        let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        menu_box.set_margin_all(8);
        menu_box.append(&import_btn);
        menu_box.append(&export_btn);
        menu_box.append(&backup_btn);
        menu_box.append(&sync_btn);
        menu_box.append(&settings_btn);
        menu_box.append(&quit_btn);
        let menu_popover = gtk::Popover::new();
        menu_popover.set_child(Some(&menu_box));
        let menu_btn = gtk::MenuButton::new();
        menu_btn.set_icon_name("open-menu-symbolic");
        menu_btn.set_popover(Some(&menu_popover));
        menu_btn.set_tooltip_text(Some(tr!("Menu")));

        let header = gtk::HeaderBar::new();
        header.pack_end(&menu_btn);
        header.pack_end(&mode_box);
        header.pack_end(&trash_btn);

        // ------------------------------------------------------------- layout
        // Three horizontal panes (sidebar | note list | editor) nested in two
        // GtkPaneds so both dividers can be dragged. GtkPaned keeps the
        // divider position proportional to its width across relayouts on its
        // own (both children default to resize=true, so the position is
        // scaled by the allocation change), which means resizing or
        // maximizing the window preserves the split without any bookkeeping
        // here. The only thing the app does is establish the initial 1:1:2
        // split once, from the panes' own allocated widths (never the
        // surface width, which on Wayland updates before the widgets
        // reallocate — see `wire_initial_split`).
        let middle_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        middle_pane.set_start_child(Some(&sidebar));
        middle_pane.set_end_child(Some(&middle));

        let main_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        main_pane.set_start_child(Some(&middle_pane));
        main_pane.set_end_child(Some(&editor_pane));
        wire_initial_split(&main_pane, &middle_pane);

        // AdwApplicationWindow manages its own titlebar; the header bar must
        // live inside the content instead.
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&header);
        content.append(&main_pane);
        window.set_content(Some(&content));

        // ------------------------------------------------------- shortcuts
        let controller = gtk::EventControllerKey::new();
        {
            use gtk::gdk::Key;
            let emit = emit.clone();
            let win = window.clone();
            controller.connect_key_pressed(move |_c, keyval, _code, state| {
                let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
                let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                // Ctrl+<letter> arrives as the lowercase keysym (Key::s, not
                // Key::S); normalize via unicode so caps lock can't break the
                // match either.
                let key = keyval.to_unicode().map(|c| c.to_ascii_lowercase());
                if ctrl && !shift && key == Some('s') {
                    emit(AppMsg::SaveNote);
                    return glib::Propagation::Stop;
                }
                if ctrl && !shift && key == Some('n') {
                    emit(AppMsg::NewNote);
                    return glib::Propagation::Stop;
                }
                if ctrl && !shift && key == Some('f') {
                    emit(AppMsg::FocusFind);
                    return glib::Propagation::Stop;
                }
                if ctrl && shift && key == Some('f') {
                    emit(AppMsg::FocusSearch);
                    return glib::Propagation::Stop;
                }
                if ctrl && !shift && key == Some('e') {
                    emit(AppMsg::CycleEditorMode);
                    return glib::Propagation::Stop;
                }
                if !ctrl && keyval == Key::F3 {
                    emit(AppMsg::FindNext);
                    return glib::Propagation::Stop;
                }
                if ctrl && !shift && key == Some('q') {
                    win.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        window.add_controller(controller);

        {
            let emit = app_sender.clone();
            let allow_close = allow_close.clone();
            window.connect_close_request(move |_w| {
                if allow_close.get() {
                    glib::Propagation::Proceed
                } else {
                    let _ = emit.send(AppMsg::CloseRequested);
                    glib::Propagation::Stop
                }
            });
        }

        // ------------------------------------------------------- settings
        let settings_window = build_settings_window(&settings, emit);

        // ------------------------------------------------------------ model
        let widgets = Widgets {
            window,
            dirty_label,
            status_label,
            sync_label,
            save_btn,
            search_entry,
            notebook_store,
            tag_flow,
            tag_menu,
            view_title,
            notes_list,
            notes_empty,
            restore_btn,
            delete_btn,
            note_normal_box,
            note_trash_box,
            title_entry,
            editor,
            mode_source_btn,
            mode_split_btn,
            mode_preview_btn,
            tag_editor_flow,
            status_bar,
            settings_window,
        };

        let model = App {
            widgets,
            worker,
            ui_sender: app_sender,
            settings,
            mode: ViewMode::All,
            notebooks: Vec::new(),
            notes: Vec::new(),
            trashed: Vec::new(),
            tags: Vec::new(),
            note_tags: Vec::new(),
            current_note: None,
            saved_title: String::new(),
            saved_content: String::new(),
            dirty: false,
            editor_mode: EditorMode::Split,
            active_tag: None,
            selected_trashed: None,
            pending_open: None,
            pending_new_note: false,
            pending_close: false,
            pending_import: None,
            row_ids,
            tag_ids,
            loading,
            allow_close,
            suppress_selection,
            suppress_tag_toggle,
            pending_tag,
            clipboard,
        };

        // Initial data load.
        model.worker.emit(DbMsg::LoadNotebooks);
        model.worker.emit(DbMsg::LoadTags);
        model.worker.emit(DbMsg::LoadAll);

        ComponentParts { model, widgets: () }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        let app_sender = sender.input_sender().clone();
        self.handle(msg, &app_sender);
    }
}
