//! The database worker.
//!
//! Every database operation happens here, on relm4's shared tokio runtime.
//! The UI thread never touches SQLite directly; it sends [`DbMsg`] and
//! receives [`DbEvent`]s. This keeps the UI responsive no matter how big
//! the database gets.
//!
//! This worker is the application boundary: typed data-core errors are
//! converted to user-facing text here, and never leak further.

pub use crate::application::{DbEvent, DbMsg};
use crate::core::repository::Repository;
use relm4::Worker;
use relm4::prelude::*;
use sqlx::SqlitePool;

pub struct DbWorker {
    repo: Repository,
}

impl Worker for DbWorker {
    type Init = SqlitePool;
    type Input = DbMsg;
    type Output = DbEvent;

    fn init(init: Self::Init, _sender: ComponentSender<Self>) -> Self {
        Self {
            repo: Repository::new(init),
        }
    }

    fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>) {
        let repo = self.repo.clone();
        relm4::spawn(async move {
            let event = handle(repo, message).await;
            let _ = sender.output(event);
        });
    }
}

async fn handle(repo: Repository, msg: DbMsg) -> DbEvent {
    let result: crate::core::error::Result<DbEvent> = match msg {
        DbMsg::LoadNotebooks => repo.list_notebooks().await.map(DbEvent::Notebooks),
        DbMsg::LoadTags => repo.list_tags().await.map(DbEvent::Tags),
        DbMsg::LoadNotes(id) => repo.list_notes(id).await.map(DbEvent::Notes),
        DbMsg::LoadUnfiled => repo.list_unfiled_notes().await.map(DbEvent::Notes),
        DbMsg::LoadAll => repo.list_all_notes().await.map(DbEvent::Notes),
        DbMsg::LoadTrashed => repo.list_trashed().await.map(DbEvent::Trashed),
        DbMsg::LoadByTag(id) => repo.list_notes_by_tag(id).await.map(DbEvent::Notes),
        DbMsg::LoadNote(id) => match repo.get_note(id).await {
            Ok(Some(note)) => Ok(DbEvent::NoteLoaded(note)),
            Ok(None) => Err(crate::core::error::Error::NoteNotFound(id)),
            Err(e) => Err(e),
        },
        DbMsg::CreateNote(notebook_id) => repo
            .create_note(notebook_id, "Untitled")
            .await
            .map(DbEvent::NoteCreated),
        DbMsg::UpdateNote { id, title, content } => repo
            .update_note(id, &title, &content)
            .await
            .map(|_| DbEvent::NoteSaved { id }),
        DbMsg::TrashNote(id) => repo
            .trash_note(id)
            .await
            .map(|_| DbEvent::NoteTrashed { id }),
        DbMsg::RestoreNote(id) => repo
            .restore_note(id)
            .await
            .map(|_| DbEvent::NoteRestored { id }),
        DbMsg::DeleteForever(id) => repo
            .delete_note_forever(id)
            .await
            .map(|_| DbEvent::NoteDeletedForever { id }),
        DbMsg::CreateNotebook { parent, name } => repo
            .create_notebook(parent, &name)
            .await
            .map(|_| DbEvent::DataChanged),
        DbMsg::RenameNotebook { id, name } => repo
            .rename_notebook(id, &name)
            .await
            .map(|_| DbEvent::DataChanged),
        DbMsg::DeleteNotebook(id) => repo.delete_notebook(id).await.map(|_| DbEvent::DataChanged),
        DbMsg::RenameTag { id, name } => repo
            .rename_tag(id, &name)
            .await
            .map(|_| DbEvent::DataChanged),
        DbMsg::DeleteTag(id) => repo.delete_tag(id).await.map(|_| DbEvent::DataChanged),
        DbMsg::SetTags { note_id, names } => repo
            .set_note_tags(note_id, &names)
            .await
            .map(|_| DbEvent::NoteSaved { id: note_id }),
        DbMsg::LoadNoteTags(id) => repo.get_note_tags(id).await.map(DbEvent::NoteTags),
        DbMsg::Search(query) => repo.search(&query).await.map(DbEvent::SearchResults),
        DbMsg::ExportMarkdown(dir) => match repo.export_markdown(&dir).await {
            Ok(n) => Ok(DbEvent::ExportDone(Ok(n))),
            Err(e) => Ok(DbEvent::ExportDone(Err(format!("{e:#}")))),
        },
        DbMsg::Backup(dest) => match repo.backup(&dest).await {
            Ok(()) => Ok(DbEvent::BackupDone(Ok(()))),
            Err(e) => Ok(DbEvent::BackupDone(Err(format!("{e:#}")))),
        },
        DbMsg::SyncNow(settings) => {
            match crate::sync::executor::run_sync(&repo, settings.as_ref()).await {
                Ok(stats) => Ok(DbEvent::SyncDone(stats)),
                Err(e) => Ok(DbEvent::SyncFailed(e.to_string())),
            }
        }
    };

    match result {
        Ok(event) => event,
        Err(e) => DbEvent::Error(format!("{e:#}")),
    }
}
