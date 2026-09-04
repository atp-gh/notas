//! Main application component: three-pane layout (sidebar | note list |
//! editor), wires the DB worker to the widgets, and owns all app state.
//!
//! The widget tree is built imperatively in `init` and stored inside the
//! model so that message handlers can touch widgets directly.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita::prelude::*;
use relm4::prelude::*;
use relm4::{ComponentParts, ComponentSender, Controller, SimpleComponent};
use sourceview5::prelude::*;
use sqlx::SqlitePool;

use notas_core::models::{Note, Notebook, SearchHit, Tag, TagCount};

use crate::config::{Settings, ThemeMode};
use crate::db_worker::{DbEvent, DbMsg, DbWorker};
use crate::editor::{Editor, build_editor};
use crate::settings_window::build_settings_window;
use crate::tr;

type AppSender = relm4::Sender<AppMsg>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewId {
    All,
    Unfiled,
    Trash,
}

#[derive(Debug, Clone)]
pub enum AppMsg {
    Db(DbEvent),
    SelectView(ViewId),
    SelectNotebook(i64),
    SelectTag(Option<i64>),
    SelectNote(i64),
    SearchChanged(String),
    NewNote,
    NewNotebook(String),
    RenameNotebook { id: i64, name: String },
    DeleteNotebook(i64),
    RenameTag { id: i64, name: String },
    DeleteTag(i64),
    TrashNote,
    RestoreNote,
    DeleteForever,
    DeleteForeverConfirmed,
    SaveNote,
    TitleChanged,
    ContentChanged,
    TogglePreview,
    FocusSearch,
    FocusFind,
    FindChanged(String),
    FindNext,
    FindPrev,
    ReplaceAll(String),
    TagsEdited(Vec<String>),
    ExportMarkdown,
    ExportTo(PathBuf),
    BackupNow,
    BackupTo(PathBuf),
    DialogSave,
    DialogDiscard,
    DialogCancel,
    CloseRequested,
    // settings
    OpenSettings,
    ThemeChanged(ThemeMode),
    ToggleLineNumbers(bool),
    ToggleStatusBar(bool),
    // sync
    SyncNow,
    SyncEndpointChanged(String),
    SyncRegionChanged(String),
    SyncBucketChanged(String),
    SyncPrefixChanged(String),
    SyncAccessKeyChanged(String),
    SyncSecretKeyChanged(String),
}

#[derive(Debug, Clone)]
enum ViewMode {
    All,
    Unfiled,
    Trash,
    Notebook(i64),
    Tag(i64),
    Search(String),
}

/// Divider positions for the three-pane layout, kept as fractions so that a
/// window resize or maximize scales every pane proportionally instead of
/// letting one pane absorb the whole width change.
#[derive(Clone, Copy)]
struct PaneRatios {
    /// Outer divider: (sidebar + note list) / window width.
    outer: f64,
    /// Inner divider: sidebar / (sidebar + note list) width.
    inner: f64,
    /// Whether a split has been applied yet. Before that, the panes have no
    /// real geometry to sample, so the default 1:1:2 split is used.
    applied: bool,
}

impl Default for PaneRatios {
    /// The default 1:1:2 split (sidebar : note list : editor): the outer
    /// divider sits at half the window (editor gets the other half) and the
    /// inner divider at half of the left half (sidebar and note list each
    /// get a quarter).
    fn default() -> Self {
        Self {
            outer: 0.5,
            inner: 0.5,
            applied: false,
        }
    }
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
    current_note: Option<i64>,
    saved_title: String,
    saved_content: String,
    dirty: bool,
    preview: bool,
    active_tag: Option<i64>,
    selected_trashed: Option<i64>,
    pending_open: Option<i64>,
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

    #[expect(
        deprecated,
        reason = "notebook sidebar uses gtk::TreeView; migrating to gtk::ColumnView is a separate UI task"
    )]
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
        apply_theme(settings.theme.mode);

        let loading = Rc::new(Cell::new(false));
        let allow_close = Rc::new(Cell::new(false));
        let suppress_selection = Rc::new(Cell::new(false));
        let suppress_tag_toggle = Rc::new(Cell::new(false));
        let row_ids = Rc::new(RefCell::new(Vec::new()));
        let tag_ids = Rc::new(RefCell::new(Vec::new()));
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
        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some(tr!("Search notes…")));
        search_entry.set_margin_bottom(6);
        {
            let emit = emit.clone();
            search_entry.connect_search_changed(move |entry| {
                emit(AppMsg::SearchChanged(entry.text().to_string()));
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
            let emit = emit.clone();
            view_list.connect_row_selected(move |_list, row| {
                if let Some(row) = row {
                    let view = match row.widget_name().as_str() {
                        "trash" => ViewId::Trash,
                        "unfiled" => ViewId::Unfiled,
                        _ => ViewId::All,
                    };
                    emit(AppMsg::SelectView(view));
                }
            });
        }

        let notebooks_label = gtk::Label::new(Some(tr!("Notebooks")));
        notebooks_label.set_halign(gtk::Align::Start);
        notebooks_label.set_margin_top(10);
        notebooks_label.set_margin_bottom(4);
        notebooks_label.add_css_class("heading");

        let new_nb_btn = gtk::Button::from_icon_name("folder-new-symbolic");
        new_nb_btn.set_tooltip_text(Some(tr!("New notebook")));
        new_nb_btn.set_halign(gtk::Align::End);
        new_nb_btn.set_valign(gtk::Align::Center);
        {
            let win = window.clone();
            let sender = app_sender.clone();
            new_nb_btn.connect_clicked(move |_| {
                prompt_input(
                    &win,
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
        notebooks_header.append(&new_nb_btn);

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
            let emit = emit.clone();
            notebook_tree.connect_row_activated(move |tree, path, _col| {
                if let Some(model) = tree.model()
                    && let Some(iter) = model.iter(path)
                {
                    let id: i64 = model.get_value(&iter, 0).get().unwrap_or(0);
                    emit(AppMsg::SelectNotebook(id));
                }
            });
        }

        // Right-click context menu on the notebook tree: rename / delete.
        let tree_menu = gtk::Popover::new();
        let rename_nb_btn = gtk::Button::with_label(tr!("Rename…"));
        rename_nb_btn.set_halign(gtk::Align::Fill);
        let delete_nb_btn = gtk::Button::with_label(tr!("Delete notebook"));
        delete_nb_btn.set_halign(gtk::Align::Fill);
        delete_nb_btn.add_css_class("destructive-action");
        {
            let win = window.clone();
            let pending = pending_nb.clone();
            let sender = app_sender.clone();
            rename_nb_btn.connect_clicked(move |_| {
                let id = pending.get();
                prompt_input(
                    &win,
                    &sender,
                    tr!("Rename notebook"),
                    tr!("Notebook name…"),
                    "",
                    move |name| AppMsg::RenameNotebook { id, name },
                );
            });
        }
        {
            let emit = emit.clone();
            let pending = pending_nb.clone();
            delete_nb_btn.connect_clicked(move |_| {
                emit(AppMsg::DeleteNotebook(pending.get()));
            });
        }
        let tree_menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        tree_menu_box.set_margin_all(8);
        tree_menu_box.append(&rename_nb_btn);
        tree_menu_box.append(&delete_nb_btn);
        tree_menu.set_child(Some(&tree_menu_box));
        {
            let tree = notebook_tree.clone();
            let menu = tree_menu.clone();
            let pending = pending_nb.clone();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(3);
            gesture.connect_pressed(move |_g, _n, x, y| {
                if let Some((path, _col, _x, _y)) = tree.path_at_pos(x as i32, y as i32)
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
        let rename_tag_btn = gtk::Button::with_label(tr!("Rename…"));
        rename_tag_btn.set_halign(gtk::Align::Fill);
        let delete_tag_btn = gtk::Button::with_label(tr!("Delete tag"));
        delete_tag_btn.set_halign(gtk::Align::Fill);
        delete_tag_btn.add_css_class("destructive-action");
        {
            let win = window.clone();
            let pending = pending_tag.clone();
            let sender = app_sender.clone();
            rename_tag_btn.connect_clicked(move |_| {
                let id = pending.get();
                prompt_input(
                    &win,
                    &sender,
                    tr!("Rename tag"),
                    tr!("Tag name…"),
                    "",
                    move |name| AppMsg::RenameTag { id, name },
                );
            });
        }
        {
            let emit = emit.clone();
            let pending = pending_tag.clone();
            delete_tag_btn.connect_clicked(move |_| {
                emit(AppMsg::DeleteTag(pending.get()));
            });
        }
        let tag_menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        tag_menu_box.set_margin_all(8);
        tag_menu_box.append(&rename_tag_btn);
        tag_menu_box.append(&delete_tag_btn);
        tag_menu.set_child(Some(&tag_menu_box));

        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sidebar.set_width_request(230);
        sidebar.set_margin_all(8);
        sidebar.append(&search_entry);
        sidebar.append(&view_list);
        sidebar.append(&notebooks_header);
        sidebar.append(&tree_scroll);
        sidebar.append(&tags_label);
        sidebar.append(&tag_flow);

        // ------------------------------------------------------------- middle
        let view_title = gtk::Label::new(Some(tr!("All notes")));
        view_title.set_halign(gtk::Align::Start);
        view_title.add_css_class("title-2");

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
                        emit(AppMsg::SelectNote(id));
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
        let new_note_btn = gtk::Button::from_icon_name("document-new-symbolic");
        new_note_btn.set_tooltip_text(Some(tr!("New note (Ctrl+N)")));
        {
            let emit = emit.clone();
            new_note_btn.connect_clicked(move |_| emit(AppMsg::NewNote));
        }

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
        header.pack_start(&new_note_btn);
        header.pack_end(&menu_btn);
        header.pack_end(&preview_btn);
        header.pack_end(&trash_btn);

        // ------------------------------------------------------------- layout
        // Three horizontal panes (sidebar | note list | editor) nested in two
        // GtkPaneds so both dividers can be dragged. GtkPaned only stores
        // pixel positions, so on a window resize GTK would keep the old pixel
        // offset and grow just one child; `rebalance` below re-applies the
        // dividers as fractions of the new width instead. The first split
        // defaults to 1:1:2; afterwards the user's dragged proportions are
        // preserved.
        let middle_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        middle_pane.set_start_child(Some(&sidebar));
        middle_pane.set_end_child(Some(&middle));

        let main_pane = gtk::Paned::new(gtk::Orientation::Horizontal);
        main_pane.set_start_child(Some(&middle_pane));
        main_pane.set_end_child(Some(&editor_pane));

        // Runs on every width change (resize, maximize, monitor move). Width
        // notifications arrive before the new allocation, so the panes still
        // report the previous sizes here — which is exactly what lets us
        // sample the split the user last left the dividers at.
        let ratios = Rc::new(RefCell::new(PaneRatios::default()));
        let wired_surface: Rc<RefCell<Option<gtk::gdk::Surface>>> = Rc::new(RefCell::new(None));
        let rebalance: Rc<dyn Fn(i32)> = {
            let ratios = ratios.clone();
            let outer_pane = main_pane.clone();
            let inner_pane = middle_pane.clone();
            Rc::new(move |width: i32| {
                if width <= 0 {
                    return;
                }
                let mut ratios = ratios.borrow_mut();
                if ratios.applied {
                    let outer_w = outer_pane.width();
                    let inner_w = inner_pane.width();
                    if outer_w > 0 && inner_w > 0 {
                        let outer = outer_pane.position() as f64 / f64::from(outer_w);
                        let inner = inner_pane.position() as f64 / f64::from(inner_w);
                        if outer.is_finite() && inner.is_finite() {
                            ratios.outer = outer.clamp(0.05, 0.95);
                            ratios.inner = inner.clamp(0.05, 0.95);
                        }
                    }
                }
                // The inner pane spans the outer pane's start child, so its
                // width follows the outer divider; derive both pixel offsets
                // from the single new total width. GtkPaned clamps the final
                // offsets to the children's minimum sizes on allocation.
                let outer_px = (ratios.outer * f64::from(width)).round() as i32;
                let inner_px = (ratios.inner * f64::from(outer_px)).round() as i32;
                eprintln!(
                    "REBAL width={width} frac_outer={:.4} frac_inner={:.4} outer_px={outer_px} inner_px={inner_px}",
                    ratios.outer, ratios.inner
                );
                outer_pane.set_position(outer_px);
                inner_pane.set_position(inner_px);
                ratios.applied = true;
            })
        };

        {
            let rebalance = rebalance.clone();
            let wired = wired_surface.clone();
            window.connect_map(move |w| {
                let surface = w.surface();
                if let Some(surface) = &surface {
                    // Wire the width handler once per surface; a window can be
                    // mapped again (minimize/restore) and its surface can be
                    // recreated (monitor move), so track which one we hooked.
                    let mut wired = wired.borrow_mut();
                    if wired.as_ref().is_none_or(|old| old != surface) {
                        *wired = Some(surface.clone());
                        let rebalance = rebalance.clone();
                        surface.connect_width_notify(move |s| rebalance(s.width()));
                    }
                }
                let total = surface.as_ref().map_or_else(|| w.width(), |s| s.width());
                rebalance(total);
            });
        }

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
                self.mode = match view {
                    ViewId::All => ViewMode::All,
                    ViewId::Unfiled => ViewMode::Unfiled,
                    ViewId::Trash => ViewMode::Trash,
                };
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
                    unsaved_dialog(&self.widgets.window, app_sender);
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
                    unsaved_dialog(&self.widgets.window, app_sender);
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
                    confirm_dialog(
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
                let dirty = self.current_note.is_some()
                    && (self.current_title() != self.saved_title
                        || self.current_content() != self.saved_content);
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
                apply_theme(mode);
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
                    self.widgets
                        .status_label
                        .set_text(tr!("Sync: configure a bucket and keys in Settings"));
                    return;
                }
                // Save unsaved edits first so the sync sees the latest
                // content; the DB worker processes the save before the
                // sync because messages run in order.
                if self.dirty {
                    self.save_note();
                }
                self.widgets.sync_label.set_text(tr!("Syncing…"));
                self.worker.emit(DbMsg::SyncNow(self.settings.sync.clone()));
            }
            AppMsg::SyncEndpointChanged(value) => {
                self.settings.sync.endpoint = value;
                self.settings.save();
            }
            AppMsg::SyncRegionChanged(value) => {
                self.settings.sync.region = value;
                self.settings.save();
            }
            AppMsg::SyncBucketChanged(value) => {
                self.settings.sync.bucket = value;
                self.settings.save();
            }
            AppMsg::SyncPrefixChanged(value) => {
                self.settings.sync.prefix = value;
                self.settings.save();
            }
            AppMsg::SyncAccessKeyChanged(value) => {
                self.settings.sync.access_key_id = value;
                self.settings.save();
            }
            AppMsg::SyncSecretKeyChanged(value) => {
                self.settings.sync.secret_access_key = value;
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
                    unsaved_dialog(&self.widgets.window, app_sender);
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
                Err(e) => error_dialog(&self.widgets.window, &e),
            },
            DbEvent::BackupDone(result) => match result {
                Ok(()) => {
                    self.widgets.status_label.set_text(tr!("Backup created"));
                }
                Err(e) => error_dialog(&self.widgets.window, &e),
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
                error_dialog(&self.widgets.window, &e);
            }
            DbEvent::Error(e) => error_dialog(&self.widgets.window, &e),
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
        match self.mode.clone() {
            ViewMode::All => self.worker.emit(DbMsg::LoadAll),
            ViewMode::Unfiled => self.worker.emit(DbMsg::LoadUnfiled),
            ViewMode::Trash => self.worker.emit(DbMsg::LoadTrashed),
            ViewMode::Notebook(id) => self.worker.emit(DbMsg::LoadNotes(id)),
            ViewMode::Tag(id) => self.worker.emit(DbMsg::LoadByTag(id)),
            ViewMode::Search(q) => self.worker.emit(DbMsg::Search(q)),
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

    #[expect(
        deprecated,
        reason = "notebook sidebar uses gtk::TreeView; migrating to gtk::ColumnView is a separate UI task"
    )]
    fn rebuild_notebook_tree(&self) {
        self.widgets.notebook_store.clear();
        let mut iters: std::collections::HashMap<i64, gtk::TreeIter> =
            std::collections::HashMap::new();
        for nb in &self.notebooks {
            let parent = nb.parent_id.and_then(|p| iters.get(&p).cloned());
            let iter = self.widgets.notebook_store.insert_with_values(
                parent.as_ref(),
                None,
                &[(0, &nb.id), (1, &nb.name)],
            );
            iters.insert(nb.id, iter);
        }
    }

    fn rebuild_tag_flow(&self) {
        clear_flow_box(&self.widgets.tag_flow);
        let mut ids = self.tag_ids.borrow_mut();
        ids.clear();
        for tag in &self.tags {
            ids.push(tag.id);
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
                    pending.set(tag_id);
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
        clear_list_box(&self.widgets.notes_list);
        let mut ids = self.row_ids.borrow_mut();
        ids.clear();

        let (list, empty) = if matches!(self.mode, ViewMode::Trash) {
            (&self.trashed, self.trashed.is_empty())
        } else {
            (&self.notes, self.notes.is_empty())
        };
        self.widgets.notes_empty.set_visible(empty);
        for note in list {
            ids.push(note.id);
            self.widgets
                .notes_list
                .append(&note_row(&note.title, &note.updated_at));
        }
        // Release the `row_ids` guard before re-borrowing it in
        // `select_note_row`; without this the RefCell panics.
        drop(ids);

        if let Some(id) = self.current_note {
            self.select_note_row(id);
        }
    }

    fn render_search_results(&self, hits: Vec<SearchHit>) {
        clear_list_box(&self.widgets.notes_list);
        let mut ids = self.row_ids.borrow_mut();
        ids.clear();
        self.widgets.notes_empty.set_visible(hits.is_empty());
        for hit in &hits {
            ids.push(hit.id);
            self.widgets
                .notes_list
                .append(&note_row(&hit.title, &hit.snippet));
        }
        drop(ids);
    }

    fn select_note_row(&self, id: i64) {
        self.suppress_selection.set(true);
        if let Some(idx) = self.row_ids.borrow().iter().position(|&x| x == id)
            && let Some(row) = self.widgets.notes_list.row_at_index(idx as i32)
        {
            self.widgets.notes_list.select_row(Some(&row));
        }
        self.suppress_selection.set(false);
    }

    fn rebuild_tag_editor(&self) {
        clear_flow_box(&self.widgets.tag_editor_flow);
        let names: Vec<String> = self.note_tags.iter().map(|t| t.name.clone()).collect();
        for tag in &self.note_tags {
            let chip = gtk::Button::with_label(&format!("× {}", tag.name));
            chip.add_css_class("pill");
            let sender = self.ui_sender.clone();
            let names = names.clone();
            let current = self.current_note;
            let tag_name = tag.name.clone();
            chip.connect_clicked(move |_| {
                if current.is_some() {
                    let remaining: Vec<String> =
                        names.iter().filter(|n| **n != tag_name).cloned().collect();
                    let _ = sender.send(AppMsg::TagsEdited(remaining));
                }
            });
            self.widgets.tag_editor_flow.append(&chip);
        }
    }
}

// ---------------------------------------------------------------------------
// Row & dialog helpers
// ---------------------------------------------------------------------------

/// Persistent text for the status-bar sync indicator. Empty (never
/// synced) shows a hint; otherwise the local time of the last run.
fn sync_indicator_text(last_synced_at: &str) -> String {
    if last_synced_at.is_empty() {
        tr!("Never synced").to_string()
    } else {
        format!("{} {last_synced_at}", tr!("Last synced"))
    }
}

/// Remove every child row from a `ListBox`.
fn clear_list_box(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

/// Remove every child chip from a `FlowBox`.
fn clear_flow_box(flow: &gtk::FlowBox) {
    while let Some(child) = flow.first_child() {
        flow.remove(&child);
    }
}

fn note_row(title: &str, subtitle: &str) -> gtk::ListBoxRow {
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

/// Apply a color scheme to libadwaita's style manager. The editor and
/// preview follow automatically through the `dark-notify` hook wired in
/// `editor::build_editor`, so one call switches the whole app.
fn apply_theme(mode: ThemeMode) {
    let scheme = match mode {
        ThemeMode::System => adw::ColorScheme::Default,
        ThemeMode::Light => adw::ColorScheme::ForceLight,
        ThemeMode::Dark => adw::ColorScheme::ForceDark,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

fn unsaved_dialog(window: &adw::ApplicationWindow, sender: &AppSender) {
    let dialog = adw::AlertDialog::new(
        Some(tr!("Unsaved changes")),
        Some(tr!("The current note has unsaved changes.")),
    );
    dialog.add_response("cancel", tr!("Cancel"));
    dialog.add_response("discard", tr!("Discard"));
    dialog.add_response("save", tr!("Save"));
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
    let s = sender.clone();
    dialog.connect_response(None::<&str>, move |_d, resp| {
        let msg = match resp {
            "save" => AppMsg::DialogSave,
            "discard" => AppMsg::DialogDiscard,
            _ => AppMsg::DialogCancel,
        };
        let _ = s.send(msg);
    });
    dialog.present(Some(window));
}

fn confirm_dialog(
    window: &adw::ApplicationWindow,
    sender: &AppSender,
    title: &str,
    body: &str,
    msg: AppMsg,
) {
    let dialog = adw::AlertDialog::new(Some(title), Some(body));
    dialog.add_response("cancel", tr!("Cancel"));
    dialog.add_response("confirm", tr!("Delete"));
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    let s = sender.clone();
    dialog.connect_response(None::<&str>, move |_d, resp| {
        if resp == "confirm" {
            let _ = s.send(msg.clone());
        }
    });
    dialog.present(Some(window));
}

fn error_dialog(window: &adw::ApplicationWindow, message: &str) {
    let dialog = adw::AlertDialog::new(Some(tr!("Error")), Some(message));
    dialog.add_response("ok", tr!("OK"));
    dialog.set_default_response(Some("ok"));
    dialog.present(Some(window));
}

#[expect(
    deprecated,
    reason = "gtk::Dialog is deprecated since 4.10; keep until adw::AlertDialog can host custom content"
)]
fn prompt_input(
    window: &adw::ApplicationWindow,
    sender: &AppSender,
    title: &str,
    placeholder: &str,
    initial: &str,
    ok: impl Fn(String) -> AppMsg + 'static,
) {
    let dialog = gtk::Dialog::with_buttons(
        Some(title),
        Some(window),
        gtk::DialogFlags::MODAL,
        &[
            (tr!("Cancel"), gtk::ResponseType::Cancel),
            (tr!("OK"), gtk::ResponseType::Ok),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Ok);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some(placeholder));
    entry.set_text(initial);
    entry.set_activates_default(true);
    dialog.content_area().append(&entry);
    dialog.set_size_request(360, -1);
    let s = sender.clone();
    let entry2 = entry.clone();
    dialog.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Ok {
            let _ = s.send(ok(entry2.text().to_string()));
        }
        d.close();
    });
    dialog.present();
    entry.grab_focus();
}
