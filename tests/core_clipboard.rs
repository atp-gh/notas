//! Copy/cut/paste and notebook-trash integration tests.
//!
//! One behavior per test, each with its own database; the shared setup
//! only opens the pool.

use notas::core::database;
use notas::core::repository::Repository;

async fn test_repo() -> (Repository, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = database::connect(dir.path().join("test.db")).await.unwrap();
    (Repository::new(pool), dir)
}

#[tokio::test]
async fn duplicated_note_gets_copy_suffix_and_keeps_tags() {
    let (repo, _dir) = test_repo().await;
    let nb = repo.create_notebook(None, "Work").await.unwrap();
    let note = repo.create_note(Some(nb.id), "Plan").await.unwrap();
    repo.update_note(note.id, "Plan", "body").await.unwrap();
    repo.set_note_tags(note.id, &["urgent".to_string()])
        .await
        .unwrap();

    let copy = repo.duplicate_note(note.id, Some(nb.id)).await.unwrap();

    assert_eq!(copy.title, "Plan (copy)");
    assert_eq!(copy.content, "body");
    assert_ne!(copy.id, note.id);
    let tags = repo.get_note_tags(copy.id).await.unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].name, "urgent");
}

#[tokio::test]
async fn repeated_note_duplicate_numbers_the_suffix() {
    let (repo, _dir) = test_repo().await;
    let nb = repo.create_notebook(None, "Work").await.unwrap();
    let note = repo.create_note(Some(nb.id), "Plan").await.unwrap();

    repo.duplicate_note(note.id, Some(nb.id)).await.unwrap();
    let second = repo.duplicate_note(note.id, Some(nb.id)).await.unwrap();

    assert_eq!(second.title, "Plan (copy 2)");
}

#[tokio::test]
async fn moved_note_lands_in_target_notebook() {
    let (repo, _dir) = test_repo().await;
    let src = repo.create_notebook(None, "Src").await.unwrap();
    let dst = repo.create_notebook(None, "Dst").await.unwrap();
    let note = repo.create_note(Some(src.id), "Movable").await.unwrap();

    repo.move_note(note.id, Some(dst.id)).await.unwrap();

    let got = repo.get_note(note.id).await.unwrap().unwrap();
    assert_eq!(got.notebook_id, Some(dst.id));
    assert!(repo.list_notes(src.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn moved_note_to_nowhere_becomes_unfiled() {
    let (repo, _dir) = test_repo().await;
    let src = repo.create_notebook(None, "Src").await.unwrap();
    let note = repo.create_note(Some(src.id), "Movable").await.unwrap();

    repo.move_note(note.id, None).await.unwrap();

    let got = repo.get_note(note.id).await.unwrap().unwrap();
    assert_eq!(got.notebook_id, None);
}

#[tokio::test]
async fn duplicated_notebook_copies_subtree_with_notes() {
    let (repo, _dir) = test_repo().await;
    let root = repo.create_notebook(None, "Root").await.unwrap();
    let child = repo.create_notebook(Some(root.id), "Child").await.unwrap();
    repo.create_note(Some(child.id), "Nested").await.unwrap();

    let copy = repo.duplicate_notebook(root.id, None).await.unwrap();

    assert_eq!(copy.name, "Root (copy)");
    assert_eq!(copy.parent_id, None);
    let notebooks = repo.list_notebooks().await.unwrap();
    let copy_child = notebooks
        .iter()
        .find(|n| n.parent_id == Some(copy.id))
        .unwrap();
    assert_eq!(copy_child.name, "Child (copy)");
    let notes = repo.list_notes(copy_child.id).await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Nested (copy)");
}

#[tokio::test]
async fn moved_notebook_reparents_under_target() {
    let (repo, _dir) = test_repo().await;
    let a = repo.create_notebook(None, "A").await.unwrap();
    let b = repo.create_notebook(None, "B").await.unwrap();

    repo.move_notebook(a.id, Some(b.id)).await.unwrap();

    let notebooks = repo.list_notebooks().await.unwrap();
    let moved = notebooks.iter().find(|n| n.id == a.id).unwrap();
    assert_eq!(moved.parent_id, Some(b.id));
}

#[tokio::test]
async fn moving_notebook_into_its_own_subtree_is_rejected() {
    let (repo, _dir) = test_repo().await;
    let root = repo.create_notebook(None, "Root").await.unwrap();
    let child = repo.create_notebook(Some(root.id), "Child").await.unwrap();

    let err = repo
        .move_notebook(root.id, Some(child.id))
        .await
        .unwrap_err();

    assert!(matches!(err, notas::core::error::Error::InvalidInput(_)));
}

#[tokio::test]
async fn trashed_notebook_hides_subtree_and_restores_it() {
    let (repo, _dir) = test_repo().await;
    let root = repo.create_notebook(None, "Root").await.unwrap();
    let child = repo.create_notebook(Some(root.id), "Child").await.unwrap();
    let note = repo.create_note(Some(child.id), "Nested").await.unwrap();

    repo.trash_notebook(root.id).await.unwrap();

    assert!(repo.list_notebooks().await.unwrap().is_empty());
    assert!(repo.list_trashed_notebooks().await.unwrap().len() == 2);
    assert!(repo.list_all_notes().await.unwrap().is_empty());

    repo.restore_notebook(root.id).await.unwrap();

    assert_eq!(repo.list_notebooks().await.unwrap().len(), 2);
    let got = repo.get_note(note.id).await.unwrap().unwrap();
    assert!(!got.is_trashed);
}

#[tokio::test]
async fn trashed_name_does_not_block_a_new_sibling() {
    let (repo, _dir) = test_repo().await;
    let root = repo.create_notebook(None, "Root").await.unwrap();

    repo.trash_notebook(root.id).await.unwrap();
    let fresh = repo.create_notebook(None, "Root").await.unwrap();

    assert_eq!(fresh.name, "Root");
    assert_ne!(fresh.id, root.id);
}
