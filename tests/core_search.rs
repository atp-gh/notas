//! FTS5 search integration tests over a real SQLite database.
//!
//! Covers the behavior matrix from the refactor plan: empty/whitespace
//! queries, quotes, multiple words, Unicode, title vs content matching,
//! trash exclusion, and the update/delete FTS triggers.

use notas::core::database;
use notas::core::repository::Repository;
use notas::domain_notes::Note;

async fn repo() -> (Repository, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let pool = database::connect(dir.path().join("t.db")).await.unwrap();
    (Repository::new(pool), dir)
}

async fn note(repo: &Repository, title: &str, content: &str) -> Note {
    let note = repo.create_note(None, title).await.unwrap();
    repo.update_note(note.id, title, content).await.unwrap();
    repo.get_note(note.id).await.unwrap().unwrap()
}

#[tokio::test]
async fn empty_and_whitespace_queries_return_no_hits() {
    let (repo, _dir) = repo().await;
    note(&repo, "hello", "world").await;
    assert!(repo.search("").await.unwrap().is_empty());
    assert!(repo.search("   ").await.unwrap().is_empty());
}

#[tokio::test]
async fn multi_word_query_requires_all_terms() {
    let (repo, _dir) = repo().await;
    note(&repo, "budget", "the agenda covers roadmap").await;
    // Both terms appear -> hit.
    let hits = repo.search("agenda roadmap").await.unwrap();
    assert_eq!(hits.len(), 1);
    // One term missing -> no hit (implicit AND).
    assert!(repo.search("agenda missing").await.unwrap().is_empty());
}

#[tokio::test]
async fn embedded_quotes_do_not_break_the_query() {
    let (repo, _dir) = repo().await;
    note(&repo, "notes", "he said \"quoted\" loudly").await;
    let hits = repo.search(r#"say "quoted""#).await.unwrap();
    assert!(hits.is_empty(), "unrelated words must not match");
    let hits = repo.search("quoted").await.unwrap();
    assert_eq!(hits.len(), 1);
    // A bare quote in the query must not turn into a syntax error.
    let hits = repo.search(r#""quoted""#).await.unwrap();
    assert_eq!(hits.len(), 1);
}

#[tokio::test]
async fn unicode_content_is_searchable() {
    let (repo, _dir) = repo().await;
    note(&repo, "中文笔记", "今天天气很好").await;
    // The unicode61 tokenizer treats each CJK run as one token (no CJK
    // word segmentation), so whole-run queries match while arbitrary
    // sub-runs do not.
    assert_eq!(repo.search("今天天气很好").await.unwrap().len(), 1);
    assert_eq!(repo.search("中文笔记").await.unwrap().len(), 1);
    assert!(repo.search("天气").await.unwrap().is_empty());
    // Mixed-script content still tokenizes on script boundaries and
    // whitespace.
    let note = repo.create_note(None, "grüße").await.unwrap();
    repo.update_note(note.id, "grüße", "naïve café").await.unwrap();
    assert_eq!(repo.search("café").await.unwrap().len(), 1);
}

#[tokio::test]
async fn title_and_content_both_match() {
    let (repo, _dir) = repo().await;
    note(&repo, "meeting notes", "nothing useful inside").await;
    note(&repo, "unrelated", "the meeting agenda").await;

    // Title match.
    assert_eq!(repo.search("meeting notes").await.unwrap().len(), 1);
    // Content match.
    assert_eq!(repo.search("agenda").await.unwrap().len(), 1);
}

#[tokio::test]
async fn trashed_notes_are_excluded() {
    let (repo, _dir) = repo().await;
    let n = note(&repo, "secret", "hidden treasure").await;
    assert_eq!(repo.search("treasure").await.unwrap().len(), 1);
    repo.trash_note(n.id).await.unwrap();
    assert!(repo.search("treasure").await.unwrap().is_empty());
    repo.restore_note(n.id).await.unwrap();
    assert_eq!(repo.search("treasure").await.unwrap().len(), 1);
}

#[tokio::test]
async fn update_trigger_refreshes_the_index() {
    let (repo, _dir) = repo().await;
    let n = note(&repo, "doc", "original content").await;
    assert_eq!(repo.search("original").await.unwrap().len(), 1);
    repo.update_note(n.id, "doc", "replacement content")
        .await
        .unwrap();
    assert!(repo.search("original").await.unwrap().is_empty());
    assert_eq!(repo.search("replacement").await.unwrap().len(), 1);
}

#[tokio::test]
async fn delete_trigger_removes_the_entry() {
    let (repo, _dir) = repo().await;
    let n = note(&repo, "temp", "vanishing words").await;
    assert_eq!(repo.search("vanishing").await.unwrap().len(), 1);
    repo.delete_note_forever(n.id).await.unwrap();
    assert!(repo.search("vanishing").await.unwrap().is_empty());
}

#[tokio::test]
async fn snippets_mark_the_match() {
    let (repo, _dir) = repo().await;
    let content = format!("{}needle{}", "filler ".repeat(30), " filler ".repeat(30));
    note(&repo, "long doc", &content).await;
    let hits = repo.search("needle").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].snippet.contains("⟪needle⟫"),
        "snippet: {}",
        hits[0].snippet
    );
}
