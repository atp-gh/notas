//! The database worker.
//!
//! Every database operation happens here, on relm4's shared tokio runtime.
//! The UI thread never touches SQLite directly; it sends [`DbMsg`] and
//! receives [`DbEvent`]s. This keeps the UI responsive no matter how big
//! the database gets.

pub use crate::core::{DbEvent, DbMsg};
use crate::storage::repo;
use relm4::Worker;
use relm4::prelude::*;
use sqlx::SqlitePool;

pub struct DbWorker {
    pool: SqlitePool,
}

impl Worker for DbWorker {
    type Init = SqlitePool;
    type Input = DbMsg;
    type Output = DbEvent;

    fn init(init: Self::Init, _sender: ComponentSender<Self>) -> Self {
        Self { pool: init }
    }

    fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>) {
        let pool = self.pool.clone();
        relm4::spawn(async move {
            let event = handle(pool, message).await;
            let _ = sender.output(event);
        });
    }
}

async fn handle(pool: SqlitePool, msg: DbMsg) -> DbEvent {
    let result: crate::core::Result<DbEvent> = match msg {
        DbMsg::LoadNotebooks => repo::list_notebooks(&pool).await.map(DbEvent::Notebooks),
        DbMsg::LoadTags => repo::list_tags(&pool).await.map(DbEvent::Tags),
        DbMsg::LoadNotes(id) => repo::list_notes(&pool, id).await.map(DbEvent::Notes),
        DbMsg::LoadUnfiled => repo::list_unfiled_notes(&pool).await.map(DbEvent::Notes),
        DbMsg::LoadAll => repo::list_all_notes(&pool).await.map(DbEvent::Notes),
        DbMsg::LoadTrashed => repo::list_trashed(&pool).await.map(DbEvent::Trashed),
        DbMsg::LoadByTag(id) => repo::list_notes_by_tag(&pool, id).await.map(DbEvent::Notes),
        DbMsg::LoadNote(id) => match repo::get_note(&pool, id).await {
            Ok(Some(note)) => Ok(DbEvent::NoteLoaded(note)),
            Ok(None) => Err(crate::core::Error::NoteNotFound(crate::core::NoteId(id))),
            Err(e) => Err(e),
        },
        DbMsg::CreateNote(notebook_id) => repo::create_note(&pool, notebook_id, "Untitled")
            .await
            .map(DbEvent::NoteCreated),
        DbMsg::UpdateNote { id, title, content } => repo::update_note(&pool, id, &title, &content)
            .await
            .map(|_| DbEvent::NoteSaved { id }),
        DbMsg::TrashNote(id) => repo::trash_note(&pool, id)
            .await
            .map(|_| DbEvent::NoteTrashed { id }),
        DbMsg::RestoreNote(id) => repo::restore_note(&pool, id)
            .await
            .map(|_| DbEvent::NoteRestored { id }),
        DbMsg::DeleteForever(id) => repo::delete_note_forever(&pool, id)
            .await
            .map(|_| DbEvent::NoteDeletedForever { id }),
        DbMsg::CreateNotebook { parent, name } => repo::create_notebook(&pool, parent, &name)
            .await
            .map(|_| DbEvent::DataChanged),
        DbMsg::RenameNotebook { id, name } => repo::rename_notebook(&pool, id, &name)
            .await
            .map(|_| DbEvent::DataChanged),
        DbMsg::DeleteNotebook(id) => repo::delete_notebook(&pool, id)
            .await
            .map(|_| DbEvent::DataChanged),
        DbMsg::RenameTag { id, name } => repo::rename_tag(&pool, id, &name)
            .await
            .map(|_| DbEvent::DataChanged),
        DbMsg::DeleteTag(id) => repo::delete_tag(&pool, id)
            .await
            .map(|_| DbEvent::DataChanged),
        DbMsg::SetTags { note_id, names } => repo::set_note_tags(&pool, note_id, &names)
            .await
            .map(|_| DbEvent::NoteSaved { id: note_id }),
        DbMsg::LoadNoteTags(id) => repo::get_note_tags(&pool, id).await.map(DbEvent::NoteTags),
        DbMsg::Search(query) => repo::search(&pool, &query)
            .await
            .map(DbEvent::SearchResults),
        DbMsg::ExportMarkdown(dir) => match repo::export_markdown(&pool, &dir).await {
            Ok(n) => Ok(DbEvent::ExportDone(Ok(n))),
            Err(e) => Ok(DbEvent::ExportDone(Err(format!("{e:#}")))),
        },
        DbMsg::Backup(dest) => match repo::backup(&pool, &dest).await {
            Ok(()) => Ok(DbEvent::BackupDone(Ok(()))),
            Err(e) => Ok(DbEvent::BackupDone(Err(format!("{e:#}")))),
        },
        DbMsg::SyncNow(settings) => {
            match crate::sync::executor::run_sync(&pool, settings.as_ref()).await {
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
