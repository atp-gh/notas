use notas::storage::{db, repo};
use sqlx::SqlitePool;

async fn test_pool() -> (SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(dir.path().join("test.db")).await.unwrap();
    (pool, dir)
}

#[tokio::test]
async fn notebook_note_roundtrip_and_search() {
    let (pool, _dir) = test_pool().await;

    let nb = repo::create_notebook(&pool, None, "Work").await.unwrap();
    let note = repo::create_note(&pool, Some(nb.id), "Meeting notes")
        .await
        .unwrap();
    repo::update_note(
        &pool,
        note.id,
        "Meeting notes",
        "# Agenda\n- Discuss the **budget**",
    )
    .await
    .unwrap();

    let got = repo::get_note(&pool, note.id).await.unwrap().unwrap();
    assert_eq!(got.title, "Meeting notes");
    assert!(got.content.contains("budget"));

    // FTS search should find it via content
    let hits = repo::search(&pool, "budget").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, note.id);
    assert!(!hits[0].snippet.is_empty(), "snippet should be populated");

    // listing by notebook
    let listed = repo::list_notes(&pool, nb.id).await.unwrap();
    assert_eq!(listed.len(), 1);

    // search is empty for a miss
    assert!(
        repo::search(&pool, "nonexistenttermxyz")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn trash_restore_and_forever_delete() {
    let (pool, _dir) = test_pool().await;

    let note = repo::create_note(&pool, None, "To trash").await.unwrap();

    repo::trash_note(&pool, note.id).await.unwrap();
    assert!(repo::list_unfiled_notes(&pool).await.unwrap().is_empty());
    assert_eq!(repo::list_trashed(&pool).await.unwrap().len(), 1);

    repo::restore_note(&pool, note.id).await.unwrap();
    assert_eq!(repo::list_unfiled_notes(&pool).await.unwrap().len(), 1);
    assert!(repo::list_trashed(&pool).await.unwrap().is_empty());

    repo::delete_note_forever(&pool, note.id).await.unwrap();
    assert!(repo::get_note(&pool, note.id).await.unwrap().is_none());
}

#[tokio::test]
async fn tags_crud() {
    let (pool, _dir) = test_pool().await;

    let note = repo::create_note(&pool, None, "Tagged").await.unwrap();
    let names = vec!["rust".to_string(), "notes".to_string()];
    repo::set_note_tags(&pool, note.id, &names).await.unwrap();

    let tags = repo::get_note_tags(&pool, note.id).await.unwrap();
    assert_eq!(tags.len(), 2);

    // idempotent re-set
    repo::set_note_tags(&pool, note.id, &names).await.unwrap();
    assert_eq!(repo::get_note_tags(&pool, note.id).await.unwrap().len(), 2);

    let counts = repo::list_tags(&pool).await.unwrap();
    assert_eq!(counts.len(), 2);
    assert!(counts.iter().all(|t| t.note_count == 1));

    // removing a tag
    repo::set_note_tags(&pool, note.id, &["rust".to_string()])
        .await
        .unwrap();
    assert_eq!(repo::get_note_tags(&pool, note.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn export_markdown_writes_files() {
    let (pool, dir) = test_pool().await;

    let nb = repo::create_notebook(&pool, None, "My Notebook")
        .await
        .unwrap();
    let note = repo::create_note(&pool, Some(nb.id), "Hello world")
        .await
        .unwrap();
    repo::update_note(&pool, note.id, "Hello world", "Body text")
        .await
        .unwrap();

    let out = dir.path().join("export");
    let count = repo::export_markdown(&pool, &out).await.unwrap();
    assert_eq!(count, 1);

    let file = out.join("My Notebook").join("Hello world.md");
    let content = tokio::fs::read_to_string(&file).await.unwrap();
    assert!(content.contains("Body text"));
    assert!(content.starts_with("---"));
}

#[tokio::test]
async fn backup_creates_snapshot() {
    let (pool, dir) = test_pool().await;

    repo::create_note(&pool, None, "Backup me").await.unwrap();

    let dest = dir.path().join("backup/notas-backup.db");
    repo::backup(&pool, &dest).await.unwrap();
    assert!(dest.exists());

    // the backup is a valid sqlite db with our data
    let backup_pool = db::connect(&dest).await.unwrap();
    let notes = repo::list_unfiled_notes(&backup_pool).await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Backup me");
}
