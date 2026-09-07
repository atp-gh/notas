//! Main application component: three-pane layout (sidebar | note list |
//! editor), wires the DB worker to the widgets, and owns all app state.
//!
//! The widget tree is built imperatively in `init` and stored inside the
//! model so that message handlers can touch widgets directly.

pub mod db_worker;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita::prelude::*;
use relm4::prelude::*;
use relm4::{ComponentParts, ComponentSender, Controller, SimpleComponent};
use sourceview5::prelude::*;
use sqlx::SqlitePool;

use notas::core::model::{Note, NoteId, Notebook, SearchHit, Tag, TagCount, TagId};

use crate::app::db_worker::DbWorker;
use crate::core::config::{Settings, SyncType};
use crate::core::{DbEvent, DbMsg};
use crate::editor::{Editor, build_editor};
use crate::notes::{clear_flow, row as note_row};
use crate::tr;
use crate::ui::dialogs;
use crate::ui::settings::build_settings_window;
use crate::ui::status::sync_indicator_text;
use crate::ui::theme;
pub use crate::ui::{AppMsg, ViewMode};

type AppSender = relm4::Sender<AppMsg>;

/// Establish the initial 1:1:2 split (sidebar | note list | editor) once the
/// panes have their first real allocation, then leave every later resize to
/// GtkPaned's native proportional behaviour.
///
/// GtkPaned already scales the divider position with the allocation when
/// both children are resizeable (the default: `position = new_width *
/// (old_position / old_width)` in `gtk_paned_calc_position`), so the app
/// only needs to set the split once and never touch it again. But that one
/// set is required: with no position ever set, GtkPaned's first layout
/// places the divider from the children's **minimum** sizes, and a nested
/// paned's minimum is ~0 (its children default to shrinkable), so the
/// divider lands at the far left and GtkPaned overflows both children past
/// the window edges instead of clipping them. Setting the position once
/// switches it into the proportional mode above.
///
/// The split must be derived from the panes' **own allocated widths**, not
/// from the surface width: on Wayland the surface reports a new size at
/// configure time, before the widgets reallocate, so a position computed
/// from it would be interpreted against stale geometry on the next layout
/// and drift out of the window.
///
/// An idle callback is used instead of a property notification because
/// GtkWidget has no `width` property in GTK4 to hook; it reschedules until
/// the panes report real widths. The two halves must be applied in
/// sequence: the outer split is what gives the inner paned its real width,
/// so the callback parks on the outer split first and only splits the inner
/// paned once a later layout pass has allocated it (a fresh `Continue` lets
/// that re-layout happen between idle rounds).
fn wire_initial_split(outer_pane: &gtk::Paned, inner_pane: &gtk::Paned) {
    let outer_pane = outer_pane.clone();
    let inner_pane = inner_pane.clone();
    let mut outer_split = false;
    glib::idle_add_local(move || {
        if !outer_split {
            let outer_w = outer_pane.width();
            if outer_w > 100 {
                outer_pane.set_position(outer_w / 2);
                outer_split = true;
            }
            // Let the re-layout triggered by the outer split allocate the
            // inner paned before we look at its width.
            return glib::ControlFlow::Continue;
        }
        let inner_w = inner_pane.width();
        if inner_w > 100 {
            inner_pane.set_position(inner_w / 2);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

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
    preview: bool,
    active_tag: Option<TagId>,
    selected_trashed: Option<NoteId>,
    pending_open: Option<NoteId>,
    pending_new_note: bool,
    pending_close: bool,
    row_ids: Rc<RefCell<Vec<i64>>>,
    tag_ids: Rc<RefCell<Vec<i64>>>,
    loading: Rc<Cell<bool>>,
    allow_close: Rc<Cell<bool>>,
    suppress_selection: Rc<Cell<bool>>,
    suppress_tag_toggle: Rc<Cell<bool>>,
    pending_tag: Rc<Cell<i64>>,
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
    // editor
    title_entry: gtk::Entry,
    editor: Editor,
    preview_btn: gtk::ToggleButton,
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
        let sidebar_parts =
            crate::notes::sidebar::build(&window, &app_sender, &pending_nb, &pending_tag);
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
        editor_stack_area.append(&editor.stack);

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

        let preview_btn = gtk::ToggleButton::new();
        preview_btn.set_label(tr!("Preview"));
        preview_btn.set_tooltip_text(Some(tr!("Toggle Markdown preview (Ctrl+E)")));
        {
            let emit = emit.clone();
            preview_btn.connect_toggled(move |_| emit(AppMsg::TogglePreview));
        }

        let export_btn = gtk::Button::with_label(tr!("Export Markdown…"));
        let backup_btn = gtk::Button::with_label(tr!("Backup database…"));
        let sync_btn = gtk::Button::with_label(tr!("Sync now…"));
        let settings_btn = gtk::Button::with_label(tr!("Settings…"));
        let quit_btn = gtk::Button::with_label(tr!("Quit"));
        {
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
        header.pack_end(&preview_btn);
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
                    emit(AppMsg::TogglePreview);
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
        let settings_window = build_settings_window(&settings, emit.clone());

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
            title_entry,
            editor,
            preview_btn,
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
            preview: false,
            active_tag: None,
            selected_trashed: None,
            pending_open: None,
            pending_new_note: false,
            pending_close: false,
            row_ids,
            tag_ids,
            loading,
            allow_close,
            suppress_selection,
            suppress_tag_toggle,
            pending_tag,
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

// ---------------------------------------------------------------------------
// Message handling
// ---------------------------------------------------------------------------

impl App {
    fn handle(&mut self, msg: AppMsg, app_sender: &AppSender) {
        match msg {
            AppMsg::Db(event) => self.handle_db_event(event),
            AppMsg::SelectView(view) => {
                self.mode = crate::core::state::mode_for_view(view);
                self.refresh_current_list();
                self.update_view_title();
                self.update_trash_buttons();
            }
            AppMsg::SelectNotebook(id) => {
                self.mode = ViewMode::Notebook(id);
                self.worker.emit(DbMsg::LoadNotes(id));
                self.update_view_title();
                self.update_trash_buttons();
            }
            AppMsg::SelectTag(tag) => {
                self.active_tag = tag;
                self.mode = match tag {
                    Some(id) => ViewMode::Tag(id),
                    None => ViewMode::All,
                };
                self.refresh_current_list();
                self.update_view_title();
                self.update_trash_buttons();
                self.rebuild_tag_flow();
            }
            AppMsg::SelectNote(id) => {
                if matches!(self.mode, ViewMode::Trash) {
                    self.selected_trashed = Some(id);
                    self.update_trash_buttons();
                    return;
                }
                if self.current_note == Some(id) {
                    return;
                }
                if self.dirty {
                    self.pending_open = Some(id);
                    dialogs::unsaved(&self.widgets.window, app_sender);
                } else {
                    self.worker.emit(DbMsg::LoadNote(id));
                }
            }
            AppMsg::SearchChanged(query) => {
                let query = query.trim().to_string();
                if query.is_empty() {
                    self.mode = match self.active_tag {
                        Some(id) => ViewMode::Tag(id),
                        None => ViewMode::All,
                    };
                    self.refresh_current_list();
                } else {
                    self.mode = ViewMode::Search(query.clone());
                    self.worker.emit(DbMsg::Search(query));
                }
                self.update_view_title();
            }
            AppMsg::NewNote => {
                if self.dirty && self.current_note.is_some() {
                    self.pending_new_note = true;
                    dialogs::unsaved(&self.widgets.window, app_sender);
                } else {
                    self.create_note_now();
                }
            }
            AppMsg::NewNotebook(name) => {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    self.worker
                        .emit(DbMsg::CreateNotebook { parent: None, name });
                }
            }
            AppMsg::RenameNotebook { id, name } => {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    self.worker.emit(DbMsg::RenameNotebook { id, name });
                }
            }
            AppMsg::DeleteNotebook(id) => {
                self.worker.emit(DbMsg::DeleteNotebook(id));
            }
            AppMsg::RenameTag { id, name } => {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    self.worker.emit(DbMsg::RenameTag { id, name });
                }
            }
            AppMsg::DeleteTag(id) => {
                if self.active_tag == Some(id) {
                    self.active_tag = None;
                    self.mode = ViewMode::All;
                }
                self.worker.emit(DbMsg::DeleteTag(id));
            }
            AppMsg::TrashNote => {
                if let Some(id) = self.current_note {
                    self.worker.emit(DbMsg::TrashNote(id));
                }
            }
            AppMsg::RestoreNote => {
                if let Some(id) = self.selected_trashed {
                    self.worker.emit(DbMsg::RestoreNote(id));
                }
            }
            AppMsg::DeleteForever => {
                if self.selected_trashed.is_some() {
                    dialogs::confirm(
                        &self.widgets.window,
                        app_sender,
                        tr!("Delete permanently"),
                        tr!("This note will be deleted forever. This cannot be undone."),
                        AppMsg::DeleteForeverConfirmed,
                    );
                }
            }
            AppMsg::DeleteForeverConfirmed => {
                if let Some(id) = self.selected_trashed {
                    self.worker.emit(DbMsg::DeleteForever(id));
                }
            }
            AppMsg::SaveNote => self.save_note(),
            AppMsg::TitleChanged | AppMsg::ContentChanged => {
                // Decide dirtiness by comparing with the last saved state:
                // loading a note sets the entry/buffer programmatically,
                // which also fires "changed", and reverting an edit back to
                // the saved text must clear the flag.
                let dirty = crate::core::state::editor_is_dirty(
                    self.current_note,
                    &self.current_title(),
                    &self.current_content(),
                    &self.saved_title,
                    &self.saved_content,
                );
                self.set_dirty(dirty);
            }
            AppMsg::TogglePreview => {
                self.preview = !self.preview;
                self.widgets.preview_btn.set_active(self.preview);
                if self.preview {
                    self.widgets.editor.render_preview();
                }
                let name = if self.preview { "preview" } else { "source" };
                self.widgets.editor.stack.set_visible_child_name(name);
            }
            AppMsg::FocusSearch => {
                self.widgets.search_entry.grab_focus();
            }
            AppMsg::FocusFind => {
                self.widgets.editor.search_bar.set_search_mode(true);
                self.widgets.editor.search_entry.grab_focus();
            }
            AppMsg::FindChanged(text) => {
                if text.is_empty() {
                    self.widgets.editor.set_search_text(None);
                } else {
                    self.widgets.editor.set_search_text(Some(&text));
                }
            }
            AppMsg::FindNext => self.widgets.editor.find_next(),
            AppMsg::FindPrev => self.widgets.editor.find_prev(),
            AppMsg::ReplaceAll(text) => {
                if text.is_empty() {
                    return;
                }
                let n = self.widgets.editor.replace_all(&text);
                self.widgets
                    .status_label
                    .set_text(&format!("Replaced {n} matches"));
            }
            AppMsg::TagsEdited(names) => {
                if let Some(id) = self.current_note {
                    self.worker.emit(DbMsg::SetTags { note_id: id, names });
                }
            }
            AppMsg::ExportMarkdown => {
                let dialog = gtk::FileDialog::new();
                dialog.set_title(tr!("Export notes as Markdown"));
                let s = app_sender.clone();
                dialog.select_folder(
                    Some(&self.widgets.window),
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        if let Ok(file) = result
                            && let Some(path) = file.path()
                        {
                            let _ = s.send(AppMsg::ExportTo(path));
                        }
                    },
                );
            }
            AppMsg::ExportTo(path) => {
                self.worker.emit(DbMsg::ExportMarkdown(path));
            }
            AppMsg::BackupNow => {
                let dialog = gtk::FileDialog::new();
                dialog.set_title(tr!("Backup database"));
                dialog.set_initial_name(Some("notas-backup.db"));
                let s = app_sender.clone();
                dialog.save(
                    Some(&self.widgets.window),
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        if let Ok(file) = result
                            && let Some(path) = file.path()
                        {
                            let _ = s.send(AppMsg::BackupTo(path));
                        }
                    },
                );
            }
            AppMsg::BackupTo(path) => {
                self.worker.emit(DbMsg::Backup(path));
            }
            AppMsg::OpenSettings => {
                // Rebuild from the current settings so the dialog always
                // reflects the latest values, including the last sync time
                // and any config edited since it was first opened.
                let emit = {
                    let sender = app_sender.clone();
                    move |msg| {
                        let _ = sender.send(msg);
                    }
                };
                self.widgets.settings_window = build_settings_window(&self.settings, emit);
                self.widgets
                    .settings_window
                    .present(Some(&self.widgets.window));
            }
            AppMsg::ThemeChanged(mode) => {
                self.settings.theme.mode = mode;
                theme::apply(mode);
                self.settings.save();
            }
            AppMsg::ToggleLineNumbers(on) => {
                self.settings.editor.show_line_numbers = on;
                self.widgets.editor.source_view.set_show_line_numbers(on);
                self.settings.save();
            }
            AppMsg::ToggleStatusBar(on) => {
                self.settings.interface.show_status_bar = on;
                self.widgets.status_bar.set_visible(on);
                self.settings.save();
            }
            AppMsg::SyncNow => {
                if !self.settings.sync.is_configured() {
                    let hint = match self.settings.sync.kind {
                        SyncType::S3 => {
                            tr!("Sync: configure a bucket and keys in Settings")
                        }
                        SyncType::WebDAV => {
                            tr!("Sync: configure the server URL and password in Settings")
                        }
                    };
                    self.widgets.status_label.set_text(hint);
                    return;
                }
                // The sync engine enforces this too; fail fast here so the
                // user hears it in the status bar instead of a sync error.
                if self.settings.sync.encryption.enabled
                    && self.settings.sync.encryption.password.trim().len()
                        < crate::sync::crypto::MIN_PASSWORD_LEN
                {
                    self.widgets.status_label.set_text(tr!(
                        "Sync: the encryption password must be at least 8 characters"
                    ));
                    return;
                }
                // Save unsaved edits first so the sync sees the latest
                // content; the DB worker processes the save before the
                // sync because messages run in order.
                if self.dirty {
                    self.save_note();
                }
                self.widgets.sync_label.set_text(tr!("Syncing…"));
                self.worker
                    .emit(DbMsg::SyncNow(Box::new(self.settings.sync.clone())));
            }
            AppMsg::SyncTypeChanged(kind) => {
                self.settings.sync.kind = kind;
                self.settings.save();
            }
            AppMsg::SyncEndpointChanged(value) => {
                self.settings.sync.s3.endpoint = value;
                self.settings.save();
            }
            AppMsg::SyncRegionChanged(value) => {
                self.settings.sync.s3.region = value;
                self.settings.save();
            }
            AppMsg::SyncBucketChanged(value) => {
                self.settings.sync.s3.bucket = value;
                self.settings.save();
            }
            AppMsg::SyncPrefixChanged(value) => {
                self.settings.sync.s3.prefix = value;
                self.settings.save();
            }
            AppMsg::SyncAccessKeyChanged(value) => {
                self.settings.sync.s3.access_key_id = value;
                self.settings.save();
            }
            AppMsg::SyncSecretKeyChanged(value) => {
                self.settings.sync.s3.secret_access_key = value;
                self.settings.save();
            }
            AppMsg::SyncUrlChanged(value) => {
                self.settings.sync.webdav.url = value;
                self.settings.save();
            }
            AppMsg::SyncDirectoryChanged(value) => {
                self.settings.sync.webdav.directory = value;
                self.settings.save();
            }
            AppMsg::SyncUsernameChanged(value) => {
                self.settings.sync.webdav.username = value;
                self.settings.save();
            }
            AppMsg::SyncPasswordChanged(value) => {
                self.settings.sync.webdav.password = value;
                self.settings.save();
            }
            AppMsg::SyncInsecureTlsChanged(on) => {
                self.settings.sync.webdav.insecure_tls = on;
                self.settings.save();
            }
            AppMsg::SyncEncryptionEnabledChanged(on) => {
                self.settings.sync.encryption.enabled = on;
                self.settings.save();
            }
            AppMsg::SyncEncryptionPasswordChanged(value) => {
                self.settings.sync.encryption.password = value;
                self.settings.save();
            }
            AppMsg::DialogSave => self.save_note(),
            AppMsg::DialogDiscard => {
                self.dirty = false;
                self.set_dirty(false);
                if self.pending_close {
                    self.finish_close();
                } else if self.pending_new_note {
                    self.pending_new_note = false;
                    self.create_note_now();
                } else if let Some(id) = self.pending_open.take() {
                    self.worker.emit(DbMsg::LoadNote(id));
                }
            }
            AppMsg::DialogCancel => {
                self.pending_open = None;
                self.pending_new_note = false;
                self.pending_close = false;
            }
            AppMsg::CloseRequested => {
                if self.dirty {
                    self.pending_close = true;
                    dialogs::unsaved(&self.widgets.window, app_sender);
                } else {
                    self.finish_close();
                }
            }
        }
    }

    fn handle_db_event(&mut self, event: DbEvent) {
        match event {
            DbEvent::Notebooks(list) => {
                self.notebooks = list;
                self.rebuild_notebook_tree();
            }
            DbEvent::Tags(list) => {
                self.tags = list;
                self.rebuild_tag_flow();
            }
            DbEvent::Notes(list) => {
                self.notes = list;
                self.rebuild_notes_list();
            }
            DbEvent::Trashed(list) => {
                self.trashed = list;
                self.rebuild_notes_list();
            }
            DbEvent::NoteLoaded(note) => {
                self.current_note = Some(note.id);
                self.saved_title = note.title.clone();
                self.saved_content = note.content.clone();
                self.dirty = false;
                self.widgets.title_entry.set_text(&note.title);
                self.loading.set(true);
                self.widgets.editor.source_buffer.set_text(&note.content);
                self.loading.set(false);
                if self.preview {
                    self.widgets.editor.render_preview();
                }
                self.widgets.save_btn.set_sensitive(true);
                self.widgets.status_label.set_text("");
                self.set_dirty(false);
                self.worker.emit(DbMsg::LoadNoteTags(note.id));
                self.select_note_row(note.id);
            }
            DbEvent::NoteCreated(note) => {
                self.worker.emit(DbMsg::LoadNote(note.id));
                self.refresh_current_list();
            }
            DbEvent::NoteSaved { id } => {
                if self.pending_close {
                    self.finish_close();
                    return;
                }
                if self.pending_new_note {
                    self.pending_new_note = false;
                    self.create_note_now();
                    return;
                }
                if let Some(pending) = self.pending_open.take() {
                    self.worker.emit(DbMsg::LoadNote(pending));
                    return;
                }
                if self.current_note == Some(id) {
                    self.dirty = false;
                    self.set_dirty(false);
                    self.widgets.status_label.set_text(tr!("Saved"));
                    self.worker.emit(DbMsg::LoadNoteTags(id));
                    self.worker.emit(DbMsg::LoadTags);
                    self.refresh_current_list();
                }
            }
            DbEvent::NoteTrashed { id } => {
                if self.current_note == Some(id) {
                    self.current_note = None;
                    self.widgets.title_entry.set_text("");
                    self.loading.set(true);
                    self.widgets.editor.source_buffer.set_text("");
                    self.loading.set(false);
                    self.widgets.save_btn.set_sensitive(false);
                    self.set_dirty(false);
                }
                self.refresh_current_list();
            }
            DbEvent::NoteRestored { id } | DbEvent::NoteDeletedForever { id } => {
                // Drop the note from our cached trash list so the selection
                // state stays consistent until the reload below lands.
                self.trashed.retain(|n| n.id != id);
                if self.selected_trashed == Some(id) {
                    self.selected_trashed = None;
                }
                self.refresh_current_list();
            }
            DbEvent::DataChanged => {
                self.worker.emit(DbMsg::LoadNotebooks);
                self.worker.emit(DbMsg::LoadTags);
                self.refresh_current_list();
            }
            DbEvent::NoteTags(tags) => {
                self.note_tags = tags;
                self.rebuild_tag_editor();
            }
            DbEvent::SearchResults(hits) => {
                self.render_search_results(hits);
            }
            DbEvent::ExportDone(result) => match result {
                Ok(n) => {
                    self.widgets
                        .status_label
                        .set_text(&format!("Exported {n} notes"));
                }
                Err(e) => dialogs::error(&self.widgets.window, &e),
            },
            DbEvent::BackupDone(result) => match result {
                Ok(()) => {
                    self.widgets.status_label.set_text(tr!("Backup created"));
                }
                Err(e) => dialogs::error(&self.widgets.window, &e),
            },
            DbEvent::SyncDone(stats) => {
                self.settings.sync.last_synced_at = stats.last_synced_at.clone();
                self.settings.save();
                self.widgets
                    .sync_label
                    .set_text(&sync_indicator_text(&self.settings.sync.last_synced_at));
                let mut parts = Vec::new();
                if stats.uploaded > 0 {
                    parts.push(format!("{} {}", stats.uploaded, tr!("up")));
                }
                if stats.downloaded > 0 {
                    parts.push(format!("{} {}", stats.downloaded, tr!("down")));
                }
                if stats.trashed > 0 {
                    parts.push(format!("{} {}", stats.trashed, tr!("trashed")));
                }
                if stats.conflicts > 0 {
                    parts.push(format!("{} {}", stats.conflicts, tr!("conflicts")));
                }
                let detail = if parts.is_empty() {
                    tr!("Nothing to sync").to_string()
                } else {
                    parts.join(", ")
                };
                self.widgets.status_label.set_text(&detail);
                // Downloaded/trashed notes changed the lists; reload them.
                self.worker.emit(DbMsg::LoadNotebooks);
                self.worker.emit(DbMsg::LoadTags);
                self.refresh_current_list();
            }
            DbEvent::SyncFailed(e) => {
                // Back to the last successful sync; the error dialog carries
                // the details.
                self.widgets
                    .sync_label
                    .set_text(&sync_indicator_text(&self.settings.sync.last_synced_at));
                self.widgets.status_label.set_text(tr!("Sync failed"));
                dialogs::error(&self.widgets.window, &e);
            }
            DbEvent::Error(e) => dialogs::error(&self.widgets.window, &e),
        }
    }

    // ------------------------------------------------------------ actions

    fn create_note_now(&mut self) {
        let notebook_id = match &self.mode {
            ViewMode::Notebook(id) => Some(*id),
            _ => None,
        };
        self.worker.emit(DbMsg::CreateNote(notebook_id));
    }

    fn save_note(&mut self) {
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

    fn current_title(&self) -> String {
        self.widgets.title_entry.text().to_string()
    }

    fn current_content(&self) -> String {
        let buffer = &self.widgets.editor.source_buffer;
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .to_string()
    }

    fn resolve_saved(&mut self) {
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

    fn finish_close(&mut self) {
        self.allow_close.set(true);
        self.widgets.window.close();
    }

    fn set_dirty(&mut self, dirty: bool) {
        self.dirty = dirty;
        if dirty {
            self.widgets.dirty_label.set_text(tr!("● Unsaved changes"));
            self.widgets.dirty_label.add_css_class("error");
        } else {
            self.widgets.dirty_label.set_text(tr!("All changes saved"));
            self.widgets.dirty_label.remove_css_class("error");
        }
    }

    fn refresh_current_list(&mut self) {
        match &self.mode {
            ViewMode::All => self.worker.emit(DbMsg::LoadAll),
            ViewMode::Unfiled => self.worker.emit(DbMsg::LoadUnfiled),
            ViewMode::Trash => self.worker.emit(DbMsg::LoadTrashed),
            ViewMode::Notebook(id) => self.worker.emit(DbMsg::LoadNotes(*id)),
            ViewMode::Tag(id) => self.worker.emit(DbMsg::LoadByTag(*id)),
            ViewMode::Search(q) => self.worker.emit(DbMsg::Search(q.clone())),
        }
    }

    fn update_view_title(&self) {
        let title = match &self.mode {
            ViewMode::All => tr!("All notes").to_string(),
            ViewMode::Unfiled => tr!("Unfiled").to_string(),
            ViewMode::Trash => tr!("Trash").to_string(),
            ViewMode::Notebook(id) => self
                .notebooks
                .iter()
                .find(|n| n.id == *id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| tr!("Notebook").to_string()),
            ViewMode::Tag(id) => self
                .tags
                .iter()
                .find(|t| t.id == *id)
                .map(|t| format!("#{}", t.name))
                .unwrap_or_else(|| tr!("Tag").to_string()),
            ViewMode::Search(q) => format!("{}: {q}", tr!("Search")),
        };
        self.widgets.view_title.set_text(&title);
    }

    fn update_trash_buttons(&self) {
        let trash = matches!(self.mode, ViewMode::Trash);
        self.widgets.restore_btn.set_visible(trash);
        self.widgets.delete_btn.set_visible(trash);
        self.widgets
            .restore_btn
            .set_sensitive(self.selected_trashed.is_some());
        self.widgets
            .delete_btn
            .set_sensitive(self.selected_trashed.is_some());
    }

    // ------------------------------------------------------------ rebuilds

    fn rebuild_notebook_tree(&self) {
        crate::notes::notebook::rebuild_tree(&self.widgets.notebook_store, &self.notebooks);
    }

    fn rebuild_tag_flow(&self) {
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
                    menu.set_parent(&chip_widget);
                    menu.present();
                });
                chip.add_controller(gesture);
            }
            self.widgets.tag_flow.append(&chip);
        }
        drop(ids);
    }

    fn rebuild_notes_list(&self) {
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
    }

    fn render_search_results(&self, hits: Vec<SearchHit>) {
        crate::notes::list::render_search(
            &self.widgets.notes_list,
            &self.widgets.notes_empty,
            &self.row_ids,
            &hits,
            note_row,
        );
    }

    fn select_note_row(&self, id: NoteId) {
        self.suppress_selection.set(true);
        if let Some(idx) = self.row_ids.borrow().iter().position(|&x| x == id.0)
            && let Some(row) = self.widgets.notes_list.row_at_index(idx as i32)
        {
            self.widgets.notes_list.select_row(Some(&row));
        }
        self.suppress_selection.set(false);
    }

    fn rebuild_tag_editor(&self) {
        crate::notes::tag_editor::render(
            &self.widgets.tag_editor_flow,
            &self.note_tags,
            self.current_note.map(|id| id.0),
            &self.ui_sender,
        );
    }
}

// ---------------------------------------------------------------------------
// Row & dialog helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the three-pane layout (sidebar | note list | editor) with the
    /// same widget types and size requests as `init`, wired with
    /// [`wire_initial_split`], inside a plain window.
    fn build_probe_window() -> (gtk::Window, gtk::Paned, gtk::Paned) {
        let window = gtk::Window::new();

        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.set_width_request(230);
        sidebar.set_margin_all(8);
        sidebar.append(&gtk::SearchEntry::new());
        sidebar.append(&gtk::ListBox::new());

        let middle = gtk::Box::new(gtk::Orientation::Vertical, 6);
        middle.set_width_request(300);
        middle.set_margin_all(8);
        middle.append(&gtk::Label::new(Some("notes")));
        middle.append(&gtk::ListBox::new());

        let editor = gtk::Box::new(gtk::Orientation::Vertical, 0);
        editor.set_margin_all(8);
        let buf = sourceview5::Buffer::new(None);
        let view = sourceview5::View::new();
        view.set_buffer(Some(&buf));
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&view));
        scroll.set_vexpand(true);
        editor.append(&scroll);

        let middle_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        middle_pane.set_start_child(Some(&sidebar));
        middle_pane.set_end_child(Some(&middle));
        let main_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        main_pane.set_start_child(Some(&middle_pane));
        main_pane.set_end_child(Some(&editor));
        wire_initial_split(&main_pane, &middle_pane);

        window.set_child(Some(&main_pane));
        (window, main_pane, middle_pane)
    }
    /// Run main-loop iterations for `ms` milliseconds. A periodic ticker
    /// source is registered first so `iteration(true)` can never block on an
    /// empty event queue — it always has the ticker to dispatch, and idle
    /// sources (like `wire_initial_split`) get to run in between.
    fn pump_ms(ctx: &glib::MainContext, ms: u64) {
        let _ticker = glib::timeout_add_local(std::time::Duration::from_millis(5), || {
            glib::ControlFlow::Continue
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < deadline {
            ctx.iteration(true);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Run main-loop iterations until `cond` holds or the timeout elapses
    /// (see [`pump_ms`] for the ticker trick).
    fn pump_until(ctx: &glib::MainContext, cond: impl FnMut() -> bool, max_ms: u64) {
        let mut cond = cond;
        let _ticker = glib::timeout_add_local(std::time::Duration::from_millis(5), || {
            glib::ControlFlow::Continue
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_ms);
        while !cond() && std::time::Instant::now() < deadline {
            ctx.iteration(true);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// The panes must stay inside the window at any width: after the
    /// initial split is applied, resizing the window (smaller *and* larger,
    /// as a tiling WM does) must keep every divider within the content
    /// bounds, and a later divider drag must keep *its* proportions on the
    /// next resize. Regression test for the niri/Wayland report where the
    /// sidebar overflowed the window and then took over the whole screen on
    /// maximize.
    ///
    /// Manual probe: `cargo test --bin notas panes_stay_inside_window -- --ignored --nocapture`.
    #[test]
    #[ignore = "requires a display"]
    fn panes_stay_inside_window() {
        gtk::init().expect("gtk init");
        adw::init().expect("adw init");

        let (window, main_pane, middle_pane) = build_probe_window();
        let ctx = glib::MainContext::default();

        window.set_default_size(960, 600);
        window.present();
        // Keep pumping until the window maps **and** the initial 1:1:2 split
        // lands (the idle applies it on its own turn, which may come several
        // iterations after the map).
        pump_until(
            &ctx,
            || {
                let w = window.width();
                w > 100
                    && main_pane.width() == w
                    && (main_pane.position() - w / 2).abs() <= 3
                    && (middle_pane.position() - main_pane.position() / 2).abs() <= 3
            },
            3000,
        );
        let mut win_w = window.width();
        assert!(
            win_w > 100,
            "window should map at a real width, got {win_w}"
        );
        // The paned fills the window horizontally and the initial 1:1:2
        // split is in place.
        assert_eq!(
            main_pane.width(),
            win_w,
            "paned must match the window width"
        );
        assert!(
            (main_pane.position() - win_w / 2).abs() <= 3,
            "outer divider {} not half of {win_w}",
            main_pane.position()
        );
        assert!((middle_pane.position() - main_pane.position() / 2).abs() <= 3);

        // Resize down, up, and down again (as a tiling WM does): dividers
        // must stay inside the window and keep the 1:1:2 proportions.
        for (w, h) in [(640, 500), (1920, 900), (500, 400)] {
            window.set_default_size(w, h);
            pump_ms(&ctx, 150);

            win_w = window.width();
            assert!(win_w > 100, "window should stay mapped, got {win_w}");
            let outer = main_pane.position();
            let inner = middle_pane.position();
            assert!(
                outer > 0 && outer <= win_w,
                "outer divider {outer} outside window {win_w}"
            );
            assert!(
                inner > 0 && inner <= outer,
                "inner divider {inner} outside its paned {outer}"
            );
            assert!(
                (outer - win_w / 2).abs() <= 3,
                "outer divider {outer} not half of {win_w}"
            );
            assert!(
                (inner - outer / 2).abs() <= 3,
                "inner divider {inner} not half of {outer}"
            );
        }

        // A user drag to 60% of the window must survive the next resize.
        main_pane.set_position(win_w * 3 / 5);
        pump_ms(&ctx, 150);
        let dragged = main_pane.position();
        assert!(dragged > win_w / 2, "drag did not move the divider");

        window.set_default_size(win_w * 2, 900);
        pump_ms(&ctx, 150);
        let outer = main_pane.position();
        let win_w2 = window.width();
        let expected = win_w2 * 3 / 5;
        assert!(
            (outer - expected).abs() <= 3,
            "dragged split lost on maximize: divider {outer}, expected {expected}"
        );
    }
}
