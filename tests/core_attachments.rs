//! Attachment integration tests: resources table + blob files, Joplin
//! `_resources/` import/export round-trips, and the 100 MiB cap — through
//! the `Repository` facade.

use std::path::{Path, PathBuf};

use notas::core::database;
use notas::core::repository::Repository;
use notas::core::resources;

async fn test_repo() -> (Repository, tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let pool = database::connect(dir.path().join("t.db")).await.unwrap();
    let resources_dir = dir.path().join("resources");
    (Repository::new(pool), dir, resources_dir)
}

fn write_file(dir: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

#[tokio::test]
async fn add_file_stores_blob_and_metadata() {
    let (repo, _dir, resources_dir) = test_repo().await;
    let src_dir = tempfile::tempdir().unwrap();
    let src = write_file(src_dir.path(), "photo.png", b"fakepng-bytes");

    let res = repo
        .add_resource_file(&resources_dir, &src, "photo.png")
        .await
        .unwrap();
    assert_eq!(res.filename, "photo.png");
    assert_eq!(res.mime, "image/png");
    assert_eq!(res.size, 13);
    assert!(resources::is_valid_resource_id(&res.uuid));
    assert_eq!(
        std::fs::read(resources::resource_path(&resources_dir, &res.uuid)).unwrap(),
        b"fakepng-bytes"
    );

    let listed = repo.list_resources().await.unwrap();
    assert_eq!(listed.len(), 1);
}

#[tokio::test]
async fn oversize_attachments_are_rejected() {
    let (repo, _dir, resources_dir) = test_repo().await;
    let big = vec![0u8; (resources::MAX_ATTACHMENT_BYTES + 1) as usize];
    let err = repo
        .add_resource_bytes(&resources_dir, &big, "huge.bin")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("100 MiB"), "{err}");
    assert!(repo.list_resources().await.unwrap().is_empty());
}

#[tokio::test]
async fn export_import_round_trips_attachments_losslessly() {
    let (repo, _dir, resources_dir) = test_repo().await;
    let src_dir = tempfile::tempdir().unwrap();
    let src = write_file(src_dir.path(), "pic.png", b"png-bytes");
    let res = repo
        .add_resource_file(&resources_dir, &src, "pic.png")
        .await
        .unwrap();

    let work = repo.create_notebook(None, "Work").await.unwrap();
    let note = repo.create_note(Some(work.id), "With pic").await.unwrap();
    let body = format!("see ![pic](:/{}) here", res.uuid);
    repo.update_note(note.id, "With pic", &body).await.unwrap();

    let out = tempfile::tempdir().unwrap();
    assert_eq!(
        repo.export_markdown_with_resources(out.path(), &resources_dir)
            .await
            .unwrap(),
        1
    );
    // Single `_resources/` dir at the root with an id-prefixed name, and a
    // depth-relative link (`Work/` → `../_resources/`).
    let exported_name = format!("{}-pic.png", res.uuid);
    assert_eq!(
        std::fs::read(out.path().join("_resources").join(&exported_name)).unwrap(),
        b"png-bytes"
    );
    let md = std::fs::read_to_string(out.path().join("Work").join("With pic.md")).unwrap();
    assert!(
        md.contains(&format!("../_resources/{exported_name}")),
        "link rewritten with depth prefix: {md}"
    );

    // A fresh database receives the export back with the same resource id.
    let (repo2, _dir2, resources_dir2) = test_repo().await;
    let stats = repo2
        .import_markdown_with_resources(out.path(), &resources_dir2)
        .await
        .unwrap();
    assert_eq!(stats.notes_imported, 1);
    assert_eq!(stats.resources_imported, 1);
    assert_eq!(stats.resources_skipped, 0);
    let notes = repo2.list_all_notes().await.unwrap();
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].content.contains(&format!(":/{}", res.uuid)),
        "id preserved: {}",
        notes[0].content
    );
    assert_eq!(
        std::fs::read(resources::resource_path(&resources_dir2, &res.uuid)).unwrap(),
        b"png-bytes"
    );
}

#[tokio::test]
async fn plain_joplin_names_import_with_fresh_ids() {
    let (repo, _dir, resources_dir) = test_repo().await;
    let root = tempfile::tempdir().unwrap();
    write_file(root.path(), "_resources/picture.png", b"png");
    write_file(
        root.path(),
        "Note.md",
        b"---\ntitle: \"N\"\n---\n\n![p](_resources/picture.png)\n",
    );

    let stats = repo
        .import_markdown_with_resources(root.path(), &resources_dir)
        .await
        .unwrap();
    assert_eq!(stats.notes_imported, 1);
    assert_eq!(stats.resources_imported, 1);
    let notes = repo.list_all_notes().await.unwrap();
    let ids = resources::extract_resource_ids(&notes[0].content);
    assert_eq!(ids.len(), 1);
    assert!(resources::is_valid_resource_id(&ids[0]));
    let res = repo.get_resource(&ids[0]).await.unwrap().unwrap();
    assert_eq!(res.filename, "picture.png");
}

#[tokio::test]
async fn missing_and_oversize_blobs_skip_but_keep_the_note() {
    let (repo, _dir, resources_dir) = test_repo().await;
    let root = tempfile::tempdir().unwrap();
    write_file(
        root.path(),
        "Note.md",
        b"---\ntitle: \"N\"\n---\n\n![gone](../_resources/gone.png)\n![big](_resources/big.bin)\n",
    );
    // Sparse oversize blob: logical size past the cap, (almost) no disk use.
    let big_path = root.path().join("_resources").join("big.bin");
    std::fs::create_dir_all(big_path.parent().unwrap()).unwrap();
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&big_path)
        .unwrap();
    f.set_len(resources::MAX_ATTACHMENT_BYTES + 1).unwrap();

    let stats = repo
        .import_markdown_with_resources(root.path(), &resources_dir)
        .await
        .unwrap();
    assert_eq!(stats.notes_imported, 1);
    assert_eq!(stats.resources_imported, 0);
    assert_eq!(
        stats.resources_skipped, 3,
        "big.bin file oversize + both links dangle"
    );
    let notes = repo.list_all_notes().await.unwrap();
    assert!(notes[0].content.contains("gone.png"), "dangling link kept");
    assert!(notes[0].content.contains("big.bin"), "oversize link kept");
}

#[tokio::test]
async fn delete_resource_removes_row_and_blob() {
    let (repo, _dir, resources_dir) = test_repo().await;
    let res = repo
        .add_resource_bytes(&resources_dir, b"data", "a.txt")
        .await
        .unwrap();
    repo.delete_resource(&resources_dir, &res.uuid)
        .await
        .unwrap();
    assert!(repo.get_resource(&res.uuid).await.unwrap().is_none());
    assert!(
        !resources::resource_path(&resources_dir, &res.uuid).exists(),
        "blob removed"
    );
    // Idempotent: deleting again is not an error (GC races rely on this).
    repo.delete_resource(&resources_dir, &res.uuid)
        .await
        .unwrap();
}

#[tokio::test]
async fn import_preview_counts_resources() {
    let (repo, _dir, _res) = test_repo().await;
    let root = tempfile::tempdir().unwrap();
    write_file(root.path(), "A/one.md", b"one");
    write_file(root.path(), "_resources/x.png", b"x");
    write_file(root.path(), "_resources/y.pdf", b"y");
    let preview = repo.import_preview(root.path()).await.unwrap();
    assert_eq!(preview.notes, 1);
    assert_eq!(preview.resources, 2);
}
