//! Repository integration tests over a real SQLite database.
//!
//! One behavior per test, each with its own database; the shared setup
//! only opens the pool.

use notas::core::database;
use notas::core::model::{NotebookId, TagId};
use notas::core::repository::Repository;

async fn test_repo() -> (Repository, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = database::connect(dir.path().join("test.db")).await.unwrap();
    (Repository::new(pool), dir)
}

// ---------------------------------------------------------------- notes

#[tokio::test]
async fn created_note_roundtrips_through_get() {
    let (repo, _dir) = test_repo().await;

    let note = repo.create_note(None, "Meeting notes").await.unwrap();
    repo.update_note(note.id, "Meeting notes", "# Agenda\n- Discuss")
        .await
        .unwrap();

    let got = repo.get_note(note.id).await.unwrap().unwrap();
    assert_eq!(got.title, "Meeting notes");
    assert_eq!(got.content, "# Agenda\n- Discuss");
}

#[tokio::test]
async fn missing_note_returns_none() {
    let (repo, _dir) = test_repo().await;

    let got = repo
        .get_note(notas::core::model::NoteId(9999))
        .await
        .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn updating_missing_note_is_not_found() {
    let (repo, _dir) = test_repo().await;

    let err = repo
        .update_note(notas::core::model::NoteId(4242), "t", "c")
        .await
        .unwrap_err();
    assert!(matches!(err, notas::core::error::Error::NoteNotFound(_)));
}

#[tokio::test]
async fn notes_list_only_under_their_own_notebook() {
    let (repo, _dir) = test_repo().await;

    let nb = repo.create_notebook(None, "Work").await.unwrap();
    let other = repo.create_notebook(None, "Other").await.unwrap();
    repo.create_note(Some(nb.id), "In work").await.unwrap();

    let listed = repo.list_notes(other.id).await.unwrap();
    assert!(listed.is_empty());
    assert_eq!(repo.list_notes(nb.id).await.unwrap().len(), 1);
}

// ---------------------------------------------------------------- search

#[tokio::test]
async fn search_finds_updated_content() {
    let (repo, _dir) = test_repo().await;

    let note = repo.create_note(None, "Meeting notes").await.unwrap();
    repo.update_note(
        note.id,
        "Meeting notes",
        "# Agenda\n- Discuss the **budget**",
    )
    .await
    .unwrap();

    let hits = repo.search("budget").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, note.id);
    assert!(!hits[0].snippet.is_empty(), "snippet should be populated");
}

#[tokio::test]
async fn search_miss_returns_no_hits() {
    let (repo, _dir) = test_repo().await;

    repo.create_note(None, "Hello").await.unwrap();
    assert!(repo.search("nonexistenttermxyz").await.unwrap().is_empty());
}

// ------------------------------------------------------- trash lifecycle

#[tokio::test]
async fn trashing_hides_note_and_fills_the_trash() {
    let (repo, _dir) = test_repo().await;

    let note = repo.create_note(None, "To trash").await.unwrap();
    repo.trash_note(note.id).await.unwrap();

    assert!(repo.list_unfiled_notes().await.unwrap().is_empty());
    assert_eq!(repo.list_trashed().await.unwrap().len(), 1);
}

#[tokio::test]
async fn restoring_returns_note_to_listing() {
    let (repo, _dir) = test_repo().await;

    let note = repo.create_note(None, "To trash").await.unwrap();
    repo.trash_note(note.id).await.unwrap();
    repo.restore_note(note.id).await.unwrap();

    assert_eq!(repo.list_unfiled_notes().await.unwrap().len(), 1);
    assert!(repo.list_trashed().await.unwrap().is_empty());
}

#[tokio::test]
async fn deleting_forever_removes_the_note() {
    let (repo, _dir) = test_repo().await;

    let note = repo.create_note(None, "Gone soon").await.unwrap();
    repo.delete_note_forever(note.id).await.unwrap();

    assert!(repo.get_note(note.id).await.unwrap().is_none());
}

// ---------------------------------------------------------------- tags

#[tokio::test]
async fn set_tags_roundtrip() {
    let (repo, _dir) = test_repo().await;

    let note = repo.create_note(None, "Tagged").await.unwrap();
    let names = vec!["rust".to_string(), "notes".to_string()];
    repo.set_note_tags(note.id, &names).await.unwrap();

    let tags = repo.get_note_tags(note.id).await.unwrap();
    let got: Vec<String> = tags.iter().map(|t| t.name.clone()).collect();
    assert_eq!(got, vec!["notes", "rust"]); // sorted by name
}

#[tokio::test]
async fn re_setting_the_same_tags_is_idempotent() {
    let (repo, _dir) = test_repo().await;

    let note = repo.create_note(None, "Tagged").await.unwrap();
    let names = vec!["rust".to_string(), "notes".to_string()];
    repo.set_note_tags(note.id, &names).await.unwrap();
    repo.set_note_tags(note.id, &names).await.unwrap();

    assert_eq!(repo.get_note_tags(note.id).await.unwrap().len(), 2);
    assert_eq!(repo.list_tags().await.unwrap().len(), 2);
}

#[tokio::test]
async fn removing_a_tag_leaves_the_others() {
    let (repo, _dir) = test_repo().await;

    let note = repo.create_note(None, "Tagged").await.unwrap();
    repo.set_note_tags(note.id, &["rust".into(), "notes".into()])
        .await
        .unwrap();
    repo.set_note_tags(note.id, &["rust".to_string()])
        .await
        .unwrap();

    let tags = repo.get_note_tags(note.id).await.unwrap();
    let got: Vec<String> = tags.iter().map(|t| t.name.clone()).collect();
    assert_eq!(got, vec!["rust"]);
}

#[tokio::test]
async fn tag_counts_reflect_note_membership() {
    let (repo, _dir) = test_repo().await;

    let a = repo.create_note(None, "A").await.unwrap();
    let b = repo.create_note(None, "B").await.unwrap();
    repo.set_note_tags(a.id, &["shared".into(), "only-a".into()])
        .await
        .unwrap();
    repo.set_note_tags(b.id, &["shared".into()]).await.unwrap();

    let counts = repo.list_tags().await.unwrap();
    let count_of = |name: &str, counts: &[notas::core::model::TagCount]| {
        counts
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.note_count)
            .unwrap_or(0)
    };
    assert_eq!(count_of("shared", &counts), 2);
    assert_eq!(count_of("only-a", &counts), 1);
}

#[tokio::test]
async fn notes_by_tag_lists_only_tagged_notes() {
    let (repo, _dir) = test_repo().await;

    let a = repo.create_note(None, "Tagged note").await.unwrap();
    repo.create_note(None, "Untagged note").await.unwrap();
    repo.set_note_tags(a.id, &["rust".into()]).await.unwrap();

    let tag_id = repo.list_tags().await.unwrap()[0].id;
    let listed = repo.list_notes_by_tag(TagId(tag_id.0)).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Tagged note");
}

// ------------------------------------------------------------ notebooks

#[tokio::test]
async fn duplicate_sibling_notebook_name_is_a_typed_error() {
    let (repo, _dir) = test_repo().await;

    repo.create_notebook(None, "Work").await.unwrap();
    let err = repo.create_notebook(None, "Work").await.unwrap_err();
    assert!(matches!(
        err,
        notas::core::error::Error::NotebookNameExists(name) if name == "Work"
    ));

    // Renaming onto an existing sibling name is the same typed error.
    let other = repo.create_notebook(None, "Other").await.unwrap();
    let err = repo.rename_notebook(other.id, "Work").await.unwrap_err();
    assert!(matches!(
        err,
        notas::core::error::Error::NotebookNameExists(name) if name == "Work"
    ));
}

#[tokio::test]
async fn same_notebook_name_under_different_parents_is_allowed() {
    let (repo, _dir) = test_repo().await;

    let outer = repo.create_notebook(None, "Outer").await.unwrap();
    let nested = repo
        .create_notebook(Some(NotebookId(outer.id.0)), "Work")
        .await
        .unwrap();
    assert_eq!(nested.name, "Work");
}

#[tokio::test]
async fn notebook_names_are_trimmed_and_validated() {
    let (repo, _dir) = test_repo().await;

    // Stored trimmed; empty and separator names are rejected up front.
    let nb = repo.create_notebook(None, "  Work  ").await.unwrap();
    assert_eq!(nb.name, "Work");

    for bad in ["", "   ", "a/b", "a\\b"] {
        let err = repo.create_notebook(None, bad).await.unwrap_err();
        assert!(
            matches!(err, notas::core::error::Error::InvalidInput(_)),
            "{bad:?} must be rejected as invalid input"
        );
    }
    let err = repo.rename_notebook(nb.id, "x/y").await.unwrap_err();
    assert!(matches!(err, notas::core::error::Error::InvalidInput(_)));
}

// --------------------------------------------------------------- export

#[tokio::test]
async fn export_writes_notebook_subdirectory_with_frontmatter() {
    let (repo, dir) = test_repo().await;

    let nb = repo.create_notebook(None, "My Notebook").await.unwrap();
    let note = repo.create_note(Some(nb.id), "Hello world").await.unwrap();
    repo.update_note(note.id, "Hello world", "Body text")
        .await
        .unwrap();

    let out = dir.path().join("export");
    let count = repo.export_markdown(&out).await.unwrap();
    assert_eq!(count, 1);

    let file = out.join("My Notebook").join("Hello world.md");
    let content = tokio::fs::read_to_string(&file).await.unwrap();
    assert!(content.contains("Body text"));
    assert!(content.starts_with("---"));
}

// --------------------------------------------------------------- backup

#[tokio::test]
async fn backup_snapshot_reopens_with_the_data() {
    let (repo, dir) = test_repo().await;

    repo.create_note(None, "Backup me").await.unwrap();

    let dest = dir.path().join("backup/notas-backup.db");
    repo.backup(&dest).await.unwrap();
    assert!(dest.exists());

    let backup_pool = database::connect(&dest).await.unwrap();
    let backup_repo = Repository::new(backup_pool);
    let notes = backup_repo.list_unfiled_notes().await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Backup me");
}

#[tokio::test]
async fn notebook_helpers_still_work_for_ui() {
    let (repo, _dir) = test_repo().await;

    let nb = repo.create_notebook(None, "Work").await.unwrap();
    repo.rename_notebook(nb.id, "Projects").await.unwrap();

    let all = repo.list_notebooks().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "Projects");

    repo.delete_notebook(NotebookId(nb.id.0)).await.unwrap();
    assert!(repo.list_notebooks().await.unwrap().is_empty());
}
