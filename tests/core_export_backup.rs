//! Export and backup integration tests: filesystem I/O through the
//! `Repository` facade.

use notas::core::database;
use notas::core::repository::Repository;
use notas::domain_notes::NotebookId;

async fn repo() -> (Repository, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = database::connect(dir.path().join("t.db")).await.unwrap();
    (Repository::new(pool), dir)
}

#[tokio::test]
async fn export_writes_nested_notebook_directories() {
    let (repo, dir) = repo().await;
    let work = repo.create_notebook(None, "Work").await.unwrap();
    let year = repo
        .create_notebook(Some(NotebookId(work.id.0)), "2026")
        .await
        .unwrap();
    let note = repo.create_note(Some(year.id), "Deep note").await.unwrap();
    repo.update_note(note.id, "Deep note", "body")
        .await
        .unwrap();

    let out = dir.path().join("export");
    let count = repo.export_markdown(&out).await.unwrap();
    assert_eq!(count, 1);

    let file = out.join("Work").join("2026").join("Deep note.md");
    let content = std::fs::read_to_string(&file).unwrap();
    assert!(content.contains("body"));
    assert!(content.starts_with("---"));
}

#[tokio::test]
async fn export_unfiled_notes_go_to_the_root() {
    let (repo, dir) = repo().await;
    let note = repo.create_note(None, "Loose").await.unwrap();
    repo.update_note(note.id, "Loose", "content").await.unwrap();

    let out = dir.path().join("export");
    assert_eq!(repo.export_markdown(&out).await.unwrap(), 1);
    assert!(out.join("Loose.md").exists());
}

#[tokio::test]
async fn export_counts_collisions_per_directory() {
    let (repo, dir) = repo().await;
    let a = repo.create_notebook(None, "A").await.unwrap();
    let b = repo.create_notebook(None, "B").await.unwrap();

    // Same title in different notebooks: no collision.
    let n1 = repo.create_note(Some(a.id), "Todo").await.unwrap();
    repo.update_note(n1.id, "Todo", "a").await.unwrap();
    let n2 = repo.create_note(Some(b.id), "Todo").await.unwrap();
    repo.update_note(n2.id, "Todo", "b").await.unwrap();
    // Same title twice in the same notebook: numeric suffix.
    let n3 = repo.create_note(Some(a.id), "Todo").await.unwrap();
    repo.update_note(n3.id, "Todo", "c").await.unwrap();

    let out = dir.path().join("export");
    assert_eq!(repo.export_markdown(&out).await.unwrap(), 3);
    assert!(out.join("A").join("Todo.md").exists());
    assert!(out.join("B").join("Todo.md").exists());
    assert!(out.join("A").join("Todo-1.md").exists());
    // The unsuffixed file holds one of the two "A" notes (a or c); the
    // suffixed file holds the other. Same-title notes in B stay untouched.
    let unsuffixed = std::fs::read_to_string(out.join("A").join("Todo.md")).unwrap();
    let suffixed = std::fs::read_to_string(out.join("A").join("Todo-1.md")).unwrap();
    let bodies =
        [unsuffixed, suffixed].map(|c| c.trim_end().ends_with('a') || c.trim_end().ends_with('c'));
    assert!(bodies[0] && bodies[1], "both A notes exported: {bodies:?}");
}

#[tokio::test]
async fn export_yaml_encodes_tricky_titles() {
    let (repo, dir) = repo().await;
    let note = repo
        .create_note(None, "title: with \"quotes\" and\nnewline")
        .await
        .unwrap();
    repo.update_note(note.id, "title: with \"quotes\" and\nnewline", "x")
        .await
        .unwrap();

    let out = dir.path().join("export");
    repo.export_markdown(&out).await.unwrap();
    // Every file must be exactly one YAML document: the newline in the
    // title must not start a second frontmatter entry.
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&out)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    assert_eq!(entries.len(), 1, "one note must produce exactly one file");
    let content = std::fs::read_to_string(entries[0].path()).unwrap();
    assert_eq!(
        content.matches("---").count(),
        2,
        "frontmatter stays closed"
    );
}

#[tokio::test]
async fn backup_handles_paths_with_single_quotes() {
    let (repo, dir) = repo().await;
    repo.create_note(None, "Keep me").await.unwrap();

    let dest = dir.path().join("it's a backup's dir/notas.db");
    repo.backup(&dest).await.unwrap();
    assert!(dest.exists());

    // The snapshot is a working database with our data.
    let pool = database::connect(&dest).await.unwrap();
    let repo2 = Repository::new(pool);
    let notes = repo2.list_all_notes().await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Keep me");
}

#[tokio::test]
async fn backup_over_an_existing_file_succeeds() {
    let (repo, dir) = repo().await;
    let dest = dir.path().join("sub/notas-backup.db");
    repo.backup(&dest).await.unwrap();

    repo.create_note(None, "Added later").await.unwrap();
    // Second backup onto the existing file must overwrite, not fail.
    repo.backup(&dest).await.unwrap();

    let pool = database::connect(&dest).await.unwrap();
    let repo2 = Repository::new(pool);
    assert_eq!(repo2.list_all_notes().await.unwrap().len(), 1);
}

#[tokio::test]
async fn backup_fts_index_is_usable_after_restore() {
    let (repo, dir) = repo().await;
    let note = repo.create_note(None, "Searchable").await.unwrap();
    repo.update_note(note.id, "Searchable", "distinctive word")
        .await
        .unwrap();

    let dest = dir.path().join("backups/notas.db");
    repo.backup(&dest).await.unwrap();

    let pool = database::connect(&dest).await.unwrap();
    let repo2 = Repository::new(pool);
    let hits = repo2.search("distinctive").await.unwrap();
    assert_eq!(hits.len(), 1, "FTS must work against the backup");
    assert_eq!(hits[0].id.0, note.id.0);
}
