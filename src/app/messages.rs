//! Message and DB-event handlers of the app component.
//!
//! [`App::handle`] translates each incoming [`AppMsg`] into model updates,
//! worker requests (`DbMsg`) and dialog prompts; [`App::handle_db_event`]
//! applies worker results to the widget tree. Both delegate the actual
//! note work to the actions in [`super::actions`]. They are `pub(super)`
//! because the component's `update` (in [`super`]) routes every message
//! here.

use gtk::prelude::*;
use libadwaita::prelude::*;
use relm4::ComponentController;
use sourceview5::prelude::*;

use super::{App, AppSender};
use crate::application::config::SyncType;
use crate::application::{DbEvent, DbMsg};
use crate::notes::{Clipboard, ClipboardKind};
use crate::tr;
use crate::ui::dialogs;
use crate::ui::settings::build_settings_window;
use crate::ui::status::sync_indicator_text;
use crate::ui::theme;
use crate::ui::{AppMsg, EditorMode, ViewMode};

impl App {
    /// Mirror `mode` into the model, the editor split and the three header
    /// buttons. Early-returns when unchanged so button `toggled` signals
    /// can never recurse.
    fn set_editor_mode(&mut self, mode: EditorMode) {
        if self.editor_mode == mode && self.widgets.editor.editor_mode() == mode {
            return;
        }
        self.editor_mode = mode;
        self.widgets.editor.set_editor_mode(mode);
        // Deactivate first so only the target fires `toggled(active)`.
        for (btn, active) in [
            (&self.widgets.mode_source_btn, mode == EditorMode::Source),
            (&self.widgets.mode_split_btn, mode == EditorMode::Split),
            (&self.widgets.mode_preview_btn, mode == EditorMode::Preview),
        ] {
            if btn.is_active() != active {
                btn.set_active(active);
            }
        }
    }
    pub(super) fn handle(&mut self, msg: AppMsg, app_sender: &AppSender) {
        match msg {
            AppMsg::Db(event) => self.handle_db_event(event, app_sender),
            AppMsg::SelectView(view) => {
                self.mode = crate::application::state::mode_for_view(view);
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
                self.update_trash_buttons();
            }
            AppMsg::NewNote => {
                let target = match &self.mode {
                    ViewMode::Notebook(id) => Some(*id),
                    _ => None,
                };
                if self.dirty && self.current_note.is_some() {
                    self.pending_new_note = Some(target);
                    dialogs::unsaved(&self.widgets.window, app_sender);
                } else {
                    self.create_note_at(target);
                }
            }
            AppMsg::NewNoteAt(target) => {
                if matches!(self.mode, ViewMode::Trash) {
                    return;
                }
                if self.dirty && self.current_note.is_some() {
                    self.pending_new_note = Some(target);
                    dialogs::unsaved(&self.widgets.window, app_sender);
                } else {
                    self.create_note_at(target);
                }
            }
            AppMsg::PromptNewNotebook => {
                let parent = match &self.mode {
                    ViewMode::Notebook(id) => Some(*id),
                    _ => None,
                };
                let title = if parent.is_some() {
                    tr!("New sub-notebook")
                } else {
                    tr!("New notebook")
                };
                dialogs::input(
                    &self.widgets.window,
                    app_sender,
                    title,
                    tr!("Notebook name…"),
                    "",
                    move |name| AppMsg::NewNotebook { parent, name },
                );
            }
            AppMsg::NewNotebook { parent, name } => {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    self.worker.emit(DbMsg::CreateNotebook { parent, name });
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
            AppMsg::CopyNote(id) => {
                *self.clipboard.borrow_mut() = Some(Clipboard {
                    kind: ClipboardKind::Note,
                    id: id.0,
                    cut: false,
                });
                self.refresh_cut_dim();
            }
            AppMsg::CutNote(id) => {
                // Save first so a pending edit is not lost to the move;
                // the worker handles the save before the move.
                if self.dirty && self.current_note == Some(id) {
                    self.save_note();
                }
                *self.clipboard.borrow_mut() = Some(Clipboard {
                    kind: ClipboardKind::Note,
                    id: id.0,
                    cut: true,
                });
                self.refresh_cut_dim();
            }
            AppMsg::CopyNotebook(id) => {
                *self.clipboard.borrow_mut() = Some(Clipboard {
                    kind: ClipboardKind::Notebook,
                    id: id.0,
                    cut: false,
                });
            }
            AppMsg::CutNotebook(id) => {
                if self.dirty {
                    self.save_note();
                }
                *self.clipboard.borrow_mut() = Some(Clipboard {
                    kind: ClipboardKind::Notebook,
                    id: id.0,
                    cut: true,
                });
            }
            AppMsg::PasteToNotebook(target) => self.paste_clipboard(target),
            AppMsg::PasteToNote(note) => {
                // Trash has no paste target; the menu hides paste there,
                // and this guard keeps stray messages from creating notes.
                if matches!(self.mode, ViewMode::Trash) {
                    return;
                }
                let parent = self
                    .notes
                    .iter()
                    .find(|n| n.id == note)
                    .and_then(|n| n.notebook_id);
                self.paste_clipboard(parent);
            }
            AppMsg::TrashNoteById(id) => {
                if self.dirty && self.current_note == Some(id) {
                    self.save_note();
                }
                self.worker.emit(DbMsg::TrashNote(id));
            }
            AppMsg::TrashNotebook(id) => {
                if self.dirty {
                    self.save_note();
                }
                // Optimistically clear the editor when the open note lives
                // inside the trashed subtree; the DataChanged refresh below
                // confirms it from the database.
                if let Some(current) = self.current_note
                    && self.note_in_subtree(current, id)
                {
                    self.current_note = None;
                    self.widgets.title_entry.set_text("");
                    self.loading.set(true);
                    self.widgets.editor.source_buffer.set_text("");
                    self.loading.set(false);
                    self.widgets.save_btn.set_sensitive(false);
                    self.set_dirty(false);
                }
                self.worker.emit(DbMsg::TrashNotebook(id));
            }
            AppMsg::RestoreNotebook(id) => {
                self.worker.emit(DbMsg::RestoreNotebook(id));
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
            AppMsg::SaveNote | AppMsg::DialogSave => self.save_note(),
            AppMsg::TitleChanged | AppMsg::ContentChanged => {
                // Decide dirtiness by comparing with the last saved state:
                // loading a note sets the entry/buffer programmatically,
                // which also fires "changed", and reverting an edit back to
                // the saved text must clear the flag.
                let dirty = crate::application::state::editor_is_dirty(
                    self.current_note,
                    &self.current_title(),
                    &self.current_content(),
                    &self.saved_title,
                    &self.saved_content,
                );
                self.set_dirty(dirty);
            }
            AppMsg::SetEditorMode(mode) => {
                self.set_editor_mode(mode);
            }
            AppMsg::CycleEditorMode => {
                let next = self.editor_mode.next();
                self.set_editor_mode(next);
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
            AppMsg::ImportMarkdown => {
                let dialog = gtk::FileDialog::new();
                dialog.set_title(tr!("Import notes from Markdown (Joplin export)"));
                let s = app_sender.clone();
                dialog.select_folder(
                    Some(&self.widgets.window),
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        if let Ok(file) = result
                            && let Some(path) = file.path()
                        {
                            let _ = s.send(AppMsg::ImportFrom(path));
                        }
                    },
                );
            }
            AppMsg::ImportFrom(path) => {
                self.pending_import = Some(path.clone());
                self.worker.emit(DbMsg::ImportScan(path));
            }
            AppMsg::ImportConfirmed => {
                if let Some(path) = self.pending_import.take() {
                    self.worker.emit(DbMsg::ImportMarkdown(path));
                }
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
            AppMsg::AttachFile => {
                if self.current_note.is_none() {
                    self.widgets
                        .status_label
                        .set_text(tr!("Open a note before attaching a file"));
                    return;
                }
                let dialog = gtk::FileDialog::new();
                dialog.set_title(tr!("Attach a file"));
                let s = app_sender.clone();
                dialog.open(
                    Some(&self.widgets.window),
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        if let Ok(file) = result
                            && let Some(path) = file.path()
                        {
                            let _ = s.send(AppMsg::AttachFileSelected(path));
                        }
                    },
                );
            }
            AppMsg::AttachFileSelected(path) => {
                self.worker.emit(DbMsg::AttachFile { source: path });
            }
            AppMsg::AttachBytesSelected { filename, bytes } => {
                if self.current_note.is_none() {
                    return;
                }
                self.worker.emit(DbMsg::AttachBytes { filename, bytes });
            }
            AppMsg::AttachmentsOpen => {
                self.worker.emit(DbMsg::ListAttachments);
            }
            AppMsg::DeleteAttachment(uuid) => {
                self.worker.emit(DbMsg::DeleteAttachment { uuid });
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
                        SyncType::Webdav => {
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
            AppMsg::DialogDiscard => {
                self.dirty = false;
                self.set_dirty(false);
                if self.pending_close {
                    self.finish_close();
                } else if let Some(target) = self.pending_new_note.take() {
                    self.create_note_at(target);
                } else if let Some(id) = self.pending_open.take() {
                    self.worker.emit(DbMsg::LoadNote(id));
                }
            }
            AppMsg::DialogCancel => {
                self.pending_open = None;
                self.pending_new_note = None;
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

    fn handle_db_event(&mut self, event: DbEvent, app_sender: &AppSender) {
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
                self.saved_title.clone_from(&note.title);
                self.saved_content.clone_from(&note.content);
                self.dirty = false;
                self.widgets.title_entry.set_text(&note.title);
                self.loading.set(true);
                self.widgets.editor.reset_scroll_memory();
                self.widgets.editor.source_buffer.set_text(&note.content);
                self.loading.set(false);
                if self.editor_mode != EditorMode::Source {
                    self.widgets.editor.request_preview_immediate();
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
                if let Some(target) = self.pending_new_note.take() {
                    self.create_note_at(target);
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
                    self.widgets.editor.reset_scroll_memory();
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
                self.render_search_results(&hits);
            }
            DbEvent::ExportDone(result) => match result {
                Ok(n) => {
                    self.widgets
                        .status_label
                        .set_text(&format!("Exported {n} notes"));
                }
                Err(e) => dialogs::error(&self.widgets.window, &e),
            },
            DbEvent::ImportScanDone(result) => match result {
                Ok(preview) => {
                    let body = format!(
                        "{} {} and {} {} will be imported ({} {}). Existing notebooks with the same name will be merged.",
                        preview.notebooks,
                        tr!("notebooks"),
                        preview.notes,
                        tr!("notes"),
                        preview.resources,
                        tr!("attachments")
                    );
                    dialogs::confirm_action(
                        &self.widgets.window,
                        app_sender,
                        tr!("Import notes"),
                        &body,
                        tr!("Import"),
                        AppMsg::ImportConfirmed,
                    );
                }
                Err(e) => dialogs::error(&self.widgets.window, &e),
            },
            DbEvent::ImportDone(result) => match result {
                Ok(stats) => {
                    let mut parts = vec![format!("{} {}", stats.notes_imported, tr!("imported"))];
                    if stats.notes_updated > 0 {
                        parts.push(format!("{} {}", stats.notes_updated, tr!("updated")));
                    }
                    if stats.notes_skipped > 0 {
                        parts.push(format!("{} {}", stats.notes_skipped, tr!("skipped")));
                    }
                    if stats.resources_imported > 0 {
                        parts.push(format!(
                            "{} {}",
                            stats.resources_imported,
                            tr!("attachments")
                        ));
                    }
                    if stats.resources_skipped > 0 {
                        parts.push(format!(
                            "{} {}",
                            stats.resources_skipped,
                            tr!("attachments skipped")
                        ));
                    }
                    self.widgets.status_label.set_text(&parts.join(", "));
                    self.worker.emit(DbMsg::LoadNotebooks);
                    self.worker.emit(DbMsg::LoadTags);
                    self.refresh_current_list();
                }
                Err(e) => dialogs::error(&self.widgets.window, &e),
            },
            DbEvent::BackupDone(result) => match result {
                Ok(()) => {
                    self.widgets.status_label.set_text(tr!("Backup created"));
                }
                Err(e) => dialogs::error(&self.widgets.window, &e),
            },
            DbEvent::AttachmentAdded(result) => match result {
                Ok(res) => {
                    // Insert a Joplin-style reference at the cursor: images
                    // inline, other files as links. The buffer `changed`
                    // signal marks the note dirty and re-renders.
                    let link = if crate::core::resources::is_image_mime(&res.mime) {
                        format!("![{}](:/{})", res.filename, res.uuid)
                    } else {
                        format!("[{}](:/{})", res.filename, res.uuid)
                    };
                    self.widgets.editor.source_buffer.insert_at_cursor(&link);
                    self.widgets.status_label.set_text(&format!(
                        "{} {}",
                        tr!("Attached"),
                        res.filename
                    ));
                }
                Err(e) => dialogs::error(&self.widgets.window, &e),
            },
            DbEvent::AttachmentDeleted(result) => match result {
                Ok(uuid) => {
                    // Drop the reference from the open note (the blob/row are
                    // already gone); orphan GC covers every other note.
                    let body = self.current_content();
                    let needle = format!(":/{uuid}");
                    if body.contains(&needle) {
                        let cleaned = remove_resource_links(&body, &uuid);
                        self.widgets.editor.source_buffer.set_text(&cleaned);
                        self.handle(AppMsg::ContentChanged, app_sender);
                    }
                    self.widgets
                        .status_label
                        .set_text(tr!("Attachment removed"));
                }
                Err(e) => dialogs::error(&self.widgets.window, &e),
            },
            DbEvent::AttachmentsListed(items) => {
                // Only the current note's references, newest first already.
                let body = self.current_content();
                let ids: std::collections::HashSet<String> =
                    crate::core::resources::extract_resource_ids(&body)
                        .into_iter()
                        .collect();
                let shown: Vec<(String, String, i64)> = items
                    .into_iter()
                    .filter(|r| ids.contains(&r.uuid))
                    .map(|r| (r.uuid, r.filename, r.size))
                    .collect();
                dialogs::attachments(&self.widgets.window, app_sender, &self.resources_dir, shown);
            }
            DbEvent::SyncDone(stats) => {
                self.settings
                    .sync
                    .last_synced_at
                    .clone_from(&stats.last_synced_at);
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
                if stats.resources_uploaded + stats.resources_downloaded > 0 {
                    parts.push(format!(
                        "{} {}",
                        stats.resources_uploaded + stats.resources_downloaded,
                        tr!("attachments")
                    ));
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
}

/// Replace every Markdown link/image pointing at `:/<uuid>` with its plain
/// link text (`![alt](:/id)` → `alt`, `[text](:/id "t")` → `text`). Used
/// after an attachment is deleted so the note keeps readable text instead
/// of a dead reference.
fn remove_resource_links(body: &str, uuid: &str) -> String {
    let needle = format!("](:/{uuid}");
    let mut out = body.to_string();
    while let Some(close_bracket) = out.find(&needle) {
        // End of the destination: `)` or whitespace (titled link).
        let rest = &out[close_bracket..];
        let dest_end = rest
            .find([')', '"', ' ', '\t', '\n', '\r'])
            .map(|p| close_bracket + p)
            .unwrap_or(out.len());
        // Matching `[` before the `]` (and an optional `!` for images).
        let Some(open_bracket) = out[..close_bracket].rfind('[') else {
            break;
        };
        let link_start = if open_bracket > 0 && out.as_bytes()[open_bracket - 1] == b'!' {
            open_bracket - 1
        } else {
            open_bracket
        };
        // For titled links skip to the closing `)`.
        let link_end = if out.as_bytes().get(dest_end) == Some(&b')') {
            dest_end
        } else {
            match out[dest_end..].find(')') {
                Some(p) => dest_end + p,
                None => break,
            }
        };
        let inner = out[open_bracket + 1..close_bracket].to_string();
        out.replace_range(link_start..=link_end, &inner);
    }
    out
}
