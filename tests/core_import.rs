//! Import integration tests: filesystem tree -> database, through the
//! `Repository` facade.

use std::path::Path;

use notas::core::database;
use notas::core::repository::Repository;

async fn repo() -> (Repository, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = database::connect(dir.path().join("t.db")).await.unwrap();
    (Repository::new(pool), dir)
}

/// Write one Joplin-style note file (front matter + body) under `dir`.
fn write_joplin_note(dir: &Path, rel: &str, id: &str, title: &str, tags: &[&str], body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tags = if tags.is_empty() {
        String::new()
    } else {
        format!("tags: [{}]\n", tags.join(", "))
    };
    let content = format!(
        "---\nid: {id}\ntitle: \"{title}\"\ncreated: 2024-01-01T10:00:00.000Z\nupdated: 2024-06-15T08:30:00.000Z\n{tags}---\n\n{body}"
    );
    std::fs::write(path, content).unwrap();
}

#[tokio::test]
async fn import_creates_nested_notebook_tree_and_notes() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    write_joplin_note(
        &root,
        "Work/2026/Deep.md",
        "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6",
        "Deep note",
        &["work", "future"],
        "body text",
    );
    write_joplin_note(
        &root,
        "Personal.md",
        "b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f",
        "Personal note",
        &[],
        "personal body",
    );

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notebooks_created, 2);
    assert_eq!(stats.notes_imported, 2);

    let notebooks = repo.list_notebooks().await.unwrap();
    assert_eq!(notebooks.len(), 2);
    let work = notebooks.iter().find(|n| n.name == "Work").unwrap();
    let year = notebooks.iter().find(|n| n.name == "2026").unwrap();
    assert_eq!(year.parent_id.map(|p| p.0), Some(work.id.0));

    let notes = repo.list_notes(year.id).await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Deep note");
    assert_eq!(notes[0].content, "body text");
    // Joplin's ISO 8601 timestamps are normalized to SQLite's UTC form.
    assert_eq!(notes[0].created_at, "2024-01-01 10:00:00");
    assert_eq!(notes[0].updated_at, "2024-06-15 08:30:00");
    let tags = repo.get_note_tags(notes[0].id).await.unwrap();
    assert_eq!(
        tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        vec!["future", "work"]
    );
    // The Joplin id becomes the sync uuid (visible through the sync index).
    let index = repo.sync_local_index().await.unwrap();
    let deep = index.iter().find(|n| n.title == "Deep note").unwrap();
    assert_eq!(deep.uuid.as_str(), "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6");

    // Root-level .md files become unfiled notes.
    let unfiled = repo.list_unfiled_notes().await.unwrap();
    assert_eq!(unfiled.len(), 1);
    assert_eq!(unfiled[0].title, "Personal note");
}

#[tokio::test]
async fn import_reuses_existing_notebooks() {
    let (repo, dir) = repo().await;
    let existing = repo.create_notebook(None, "Work").await.unwrap();

    let root = dir.path().join("export");
    write_joplin_note(
        &root,
        "Work/Note.md",
        "c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f80",
        "Into existing",
        &[],
        "body",
    );

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notebooks_created, 0);
    assert_eq!(stats.notebooks_found, 1);

    let notes = repo.list_notes(existing.id).await.unwrap();
    assert_eq!(notes.len(), 1, "notes merge into the existing notebook");
}

#[tokio::test]
async fn import_updates_notes_with_the_same_joplin_id() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    let rel = "Note.md";
    let id = "d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8091";
    write_joplin_note(&root, rel, id, "First version", &["tag"], "v1 body");

    let first = repo.import_markdown(&root).await.unwrap();
    assert_eq!(first.notes_imported, 1);
    assert_eq!(first.notes_updated, 0);

    write_joplin_note(&root, rel, id, "Second version", &["tag", "new"], "v2 body");
    let second = repo.import_markdown(&root).await.unwrap();
    assert_eq!(second.notes_imported, 0);
    assert_eq!(second.notes_updated, 1);

    let notes = repo.list_all_notes().await.unwrap();
    assert_eq!(notes.len(), 1, "re-import must not duplicate the note");
    assert_eq!(notes[0].title, "Second version");
    assert_eq!(notes[0].content, "v2 body");
    let tags = repo.get_note_tags(notes[0].id).await.unwrap();
    assert_eq!(
        tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        vec!["new", "tag"]
    );
}

#[tokio::test]
async fn import_skips_unreadable_files_and_continues() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    write_joplin_note(
        &root,
        "Good.md",
        "e5f6a7b8c9d0e1f2a3b4c5d6e7f8091a2",
        "Good",
        &[],
        "ok",
    );
    // Invalid UTF-8: read_to_string fails, the note is skipped, not fatal.
    let bad = root.join("Bad.md");
    std::fs::write(bad, b"\xff\xfe\x80 not utf8").unwrap();

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notes_imported, 1);
    assert_eq!(stats.notes_skipped, 1);
    assert_eq!(repo.list_all_notes().await.unwrap().len(), 1);
}

#[tokio::test]
async fn import_ignores_resources_and_non_md_files() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    write_joplin_note(
        &root,
        "Note.md",
        "f6a7b8c9d0e1f2a3b4c5d6e7f8091a2b3",
        "Note",
        &[],
        "body",
    );
    std::fs::create_dir_all(root.join("_resources")).unwrap();
    std::fs::write(root.join("_resources/attached.png"), b"png").unwrap();
    std::fs::write(root.join("notes.txt"), "not a note").unwrap();

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notes_imported, 1);
    assert_eq!(stats.notebooks_created, 0, "_resources is not a notebook");
    let notebooks = repo.list_notebooks().await.unwrap();
    assert!(notebooks.is_empty(), "no notebook created from _resources");
    assert_eq!(repo.list_all_notes().await.unwrap().len(), 1);
}

#[tokio::test]
async fn import_empty_directory_creates_an_empty_notebook() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    std::fs::create_dir_all(root.join("Empty")).unwrap();

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notebooks_created, 1);
    assert_eq!(stats.notes_imported, 0);

    let notebooks = repo.list_notebooks().await.unwrap();
    assert_eq!(notebooks.len(), 1);
    assert_eq!(notebooks[0].name, "Empty");
}

#[tokio::test]
async fn import_without_front_matter_uses_the_file_name() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Plain title.md"), "no front matter here").unwrap();

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notes_imported, 1);
    let notes = repo.list_all_notes().await.unwrap();
    assert_eq!(notes[0].title, "Plain title");
    assert_eq!(notes[0].content, "no front matter here");
}

#[tokio::test]
async fn import_round_trips_a_notas_export() {
    let (repo, dir) = repo().await;
    let work = repo.create_notebook(None, "Work").await.unwrap();
    let year = repo.create_notebook(Some(work.id), "2026").await.unwrap();
    let note = repo.create_note(Some(year.id), "Deep note").await.unwrap();
    repo.update_note(note.id, "Deep note", "body")
        .await
        .unwrap();
    let loose = repo.create_note(None, "Loose").await.unwrap();
    repo.update_note(loose.id, "Loose", "loose body")
        .await
        .unwrap();

    let out = dir.path().join("export");
    assert_eq!(repo.export_markdown(&out).await.unwrap(), 2);

    // A fresh database receives the export back.
    let dir2 = tempfile::tempdir().unwrap();
    let pool = database::connect(dir2.path().join("t2.db")).await.unwrap();
    let repo2 = Repository::new(pool);
    let stats = repo2.import_markdown(&out).await.unwrap();
    assert_eq!(stats.notebooks_created, 2);
    assert_eq!(stats.notes_imported, 2);

    let notes = repo2.list_all_notes().await.unwrap();
    let deep = notes.iter().find(|n| n.title == "Deep note").unwrap();
    assert_eq!(deep.content, "body");
    assert_eq!(
        deep.created_at, note.created_at,
        "timestamp survives the trip"
    );
    let loose = notes.iter().find(|n| n.title == "Loose").unwrap();
    assert_eq!(loose.content, "loose body");
    assert!(loose.notebook_id.is_none(), "unfiled notes stay unfiled");
}

#[cfg(unix)]
#[tokio::test]
async fn import_skips_symlinks() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    std::fs::create_dir_all(root.join("Real")).unwrap();
    std::fs::write(root.join("Real/real.md"), "real body").unwrap();
    // A symlinked file and a symlinked directory must both be ignored
    // (the walker never follows symlinks out of the tree).
    std::os::unix::fs::symlink(root.join("Real/real.md"), root.join("Link.md")).unwrap();
    std::os::unix::fs::symlink(root.join("Real"), root.join("Linked")).unwrap();

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notes_imported, 1, "only the real file is imported");
    assert_eq!(
        stats.notebooks_created, 1,
        "the symlinked dir is not a notebook"
    );
    let notes = repo.list_all_notes().await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "real");
}

/// Relative chain `d1/d2/…/d{levels}` for building deep test trees.
fn chain_rel(levels: usize) -> String {
    let mut rel = String::from("d1");
    for i in 2..=levels {
        rel.push_str(&format!("/d{i}"));
    }
    rel
}

#[tokio::test]
async fn import_handles_deep_directory_trees() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    // A single 66-level chain; the walker stops descending at MAX_DEPTH
    // (64), so only d1..d64 become notebooks and only files at depth <= 64
    // (inside d63) are seen.
    write_joplin_note(
        &root,
        &format!("{}/inside.md", chain_rel(63)),
        "aa",
        "Inside 64",
        &[],
        "ok",
    );
    write_joplin_note(
        &root,
        &format!("{}/beyond.md", chain_rel(64)),
        "bb",
        "Beyond 64",
        &[],
        "never seen",
    );
    write_joplin_note(
        &root,
        &format!("{}/bottom.md", chain_rel(66)),
        "cc",
        "Bottom",
        &[],
        "never seen",
    );

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notebooks_created, 64, "d1..d64 become notebooks");
    assert_eq!(
        stats.notes_imported, 1,
        "only the depth-64 note is imported"
    );
    let notes = repo.list_all_notes().await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Inside 64");
}

#[tokio::test]
async fn import_moves_note_when_file_moves_notebooks() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    let id = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d";
    write_joplin_note(&root, "Work/N.md", id, "The note", &[], "v1");
    repo.import_markdown(&root).await.unwrap();

    let work = repo.list_notebooks().await.unwrap();
    let work = work.iter().find(|n| n.name == "Work").unwrap().id;
    assert_eq!(repo.list_notes(work).await.unwrap().len(), 1);

    // The note file moves to another notebook; the re-import must move
    // the existing note (matched by id) instead of duplicating it.
    std::fs::remove_file(root.join("Work/N.md")).unwrap();
    write_joplin_note(&root, "Personal/N.md", id, "The note", &[], "v2");
    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notes_updated, 1);

    let personal = repo.list_notebooks().await.unwrap();
    let personal = personal.iter().find(|n| n.name == "Personal").unwrap().id;
    assert!(repo.list_notes(work).await.unwrap().is_empty());
    let notes = repo.list_notes(personal).await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].content, "v2");
    assert_eq!(repo.list_all_notes().await.unwrap().len(), 1);
}

#[tokio::test]
async fn import_keeps_created_at_when_reimport_lacks_created() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    let id = "2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e";
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("N.md"),
        format!(
            "---\nid: {id}\ntitle: \"N\"\ncreated: 2020-05-05T05:05:05.000Z\nupdated: 2020-05-05T05:05:05.000Z\n---\n\nv1"
        ),
    )
    .unwrap();
    repo.import_markdown(&root).await.unwrap();
    let notes = repo.list_all_notes().await.unwrap();
    assert_eq!(notes[0].created_at, "2020-05-05 05:05:05");

    // Re-import without any timestamps: created_at must be preserved (the
    // update path only fills in what the front matter provides).
    std::fs::write(
        root.join("N.md"),
        format!("---\nid: {id}\ntitle: \"N\"\n---\n\nv2"),
    )
    .unwrap();
    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notes_updated, 1);
    let notes = repo.list_all_notes().await.unwrap();
    assert_eq!(
        notes[0].created_at, "2020-05-05 05:05:05",
        "created survives"
    );
    assert_ne!(
        notes[0].updated_at, "2020-05-05 05:05:05",
        "updated falls to now"
    );
}

#[tokio::test]
async fn import_merges_backslash_names_into_sanitized_notebook() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    // Notas rejects '\\' in notebook names; both directories sanitize to
    // "a-b" and must merge into one notebook, not fail.
    write_joplin_note(&root, "a\\b/N1.md", "id1", "N1", &[], "1");
    write_joplin_note(&root, "a-b/N2.md", "id2", "N2", &[], "2");

    let stats = repo.import_markdown(&root).await.unwrap();
    // The merge happens inside this run (the second path reuses the level
    // created for the first via the resolved map), so it counts as one
    // created, zero found — `found` only counts levels that existed
    // before the run.
    assert_eq!(stats.notebooks_created, 1);
    assert_eq!(stats.notebooks_found, 0);
    assert_eq!(stats.notes_imported, 2);
    let notebooks = repo.list_notebooks().await.unwrap();
    assert_eq!(notebooks.len(), 1);
    assert_eq!(notebooks[0].name, "a-b");
    assert_eq!(repo.list_notes(notebooks[0].id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn import_ignores_nested_resources_folders() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    // A `_resources` folder at any depth is skipped entirely — even when
    // it contains a `.md` file.
    write_joplin_note(&root, "Work/real.md", "id1", "Real", &[], "ok");
    write_joplin_note(
        &root,
        "Work/_resources/hidden.md",
        "id2",
        "Hidden",
        &[],
        "no",
    );

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notebooks_created, 1, "only Work");
    assert_eq!(stats.notes_imported, 1);
    let notes = repo.list_all_notes().await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Real");
}

#[tokio::test]
async fn import_preserves_unicode_notebook_names() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    write_joplin_note(&root, "工作/会议.md", "id1", "会议记录", &[], "中文内容");

    let stats = repo.import_markdown(&root).await.unwrap();
    assert_eq!(stats.notebooks_created, 1);
    assert_eq!(stats.notes_imported, 1);
    let notebooks = repo.list_notebooks().await.unwrap();
    assert_eq!(notebooks[0].name, "工作");
    let notes = repo.list_notes(notebooks[0].id).await.unwrap();
    assert_eq!(notes[0].title, "会议记录");
    assert_eq!(notes[0].content, "中文内容");
}

#[tokio::test]
async fn import_preview_counts_without_reading_files() {
    let (repo, dir) = repo().await;
    let root = dir.path().join("export");
    write_joplin_note(&root, "A/one.md", "a", "One", &[], "1");
    write_joplin_note(&root, "A/B/two.md", "b", "Two", &[], "2");
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::create_dir_all(root.join("_resources")).unwrap();
    std::fs::write(root.join("_resources/x.png"), "x").unwrap();

    let preview = repo.import_preview(&root).await.unwrap();
    assert_eq!(preview.notebooks, 3, "A, A/B and empty");
    assert_eq!(preview.notes, 2);
}
