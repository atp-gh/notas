//! Markdown export: a pure planning layer plus a filesystem writer.
//!
//! [`plan`] turns the database state into an ordered list of
//! [`PlannedFile`]s (relative directory, file name, full contents) with
//! per-directory collision counting and YAML-encoded frontmatter; [`run`]
//! then creates the directories and writes the files. Splitting the two
//! keeps the naming/escaping rules unit-testable without touching disk.

use std::collections::HashMap;
use std::path::Path;

use sqlx::SqlitePool;

use crate::core::Result;
use crate::core::model::{Note, Notebook};

/// One planned export file: a notebook-relative directory, a collision-
/// free file name, and the complete file contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Notebook directory segments, e.g. `["Work", "2026"]`; empty for
    /// unfiled notes (exported to the output root).
    pub dir: Vec<String>,
    /// File name including the `.md` extension.
    pub name: String,
    /// Full file contents (frontmatter + markdown body).
    pub content: String,
}

/// Map a display name to a safe directory/file component: characters
/// outside the portable set become `-`, and an all-stripped name falls
/// back to `untitled`.
#[must_use]
pub fn sanitize_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('-');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Encode a value for a YAML frontmatter scalar: surrounding/inner double
/// quotes are escaped and the scalar is quoted, so newlines, backslashes
/// and YAML-special characters cannot change the document structure.
#[must_use]
pub fn yaml_scalar(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Build the frontmatter block for one note. Titles are YAML-encoded; the
/// timestamps come from SQLite's fixed `YYYY-MM-DD HH:MM:SS` format and
/// need no quoting, but are quoted anyway for uniformity.
#[must_use]
pub fn frontmatter(note: &Note) -> String {
    format!(
        "---\ntitle: {}\ncreated: {}\nupdated: {}\n---\n\n",
        yaml_scalar(&note.title),
        yaml_scalar(&note.created_at),
        yaml_scalar(&note.updated_at),
    )
}

/// Compute the export plan for a set of notebooks and notes.
///
/// Notes are exported into their notebook's directory (nested notebooks
/// become nested directories); unfiled notes land in the root. File-name
/// collisions are counted **per directory**, so two notes named "Todo" in
/// different notebooks both keep their name, while a third "Todo" in the
/// same notebook becomes `Todo-1.md`.
pub fn plan(notebooks: &[Notebook], notes: &[Note]) -> Vec<PlannedFile> {
    // 1. Parent lookup by id; the plan must not depend on the input order,
    //    so a child can be processed before its parent.
    let mut parent_by_id: HashMap<i64, Option<i64>> = HashMap::new();
    for nb in notebooks {
        parent_by_id.insert(nb.id.0, nb.parent_id.map(|p| p.0));
    }

    // 2. Directory for one notebook = sanitized parent chain + own name.
    //    Cycles are broken defensively with a depth guard (the schema
    //    allows referencing any row, and the guard matches the sync-index
    //    path walker).
    let directory = |id: i64| -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = Some(id);
        let mut guard = 0;
        while let Some(nb_id) = current {
            let Some(parent) = parent_by_id.get(&nb_id) else {
                break;
            };
            let name = notebooks
                .iter()
                .find(|nb| nb.id.0 == nb_id)
                .map(|nb| sanitize_component(&nb.name));
            if let Some(name) = name {
                chain.push(name);
            }
            current = *parent;
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        chain.reverse();
        chain
    };

    let mut used_per_dir: HashMap<Vec<String>, HashMap<String, usize>> = HashMap::new();
    let mut planned = Vec::with_capacity(notes.len());
    for note in notes {
        let dir = note
            .notebook_id
            .map(|id| directory(id.0))
            .unwrap_or_default();

        let base = sanitize_component(&note.title);
        let counter = used_per_dir.entry(dir.clone()).or_default();
        let count = counter.entry(base.clone()).or_insert(0);
        let name = if *count == 0 {
            format!("{base}.md")
        } else {
            format!("{base}-{count}.md")
        };
        *count += 1;

        planned.push(PlannedFile {
            dir,
            name,
            content: format!("{}{}", frontmatter(note), note.content),
        });
    }
    planned
}

/// Query the database and write every non-trashed note as `.md` files
/// under `out_dir`. Returns the number of notes written.
pub(crate) async fn run(pool: &SqlitePool, out_dir: &Path) -> Result<usize> {
    let notebooks = notebooks::list(pool).await?;
    let notes = sqlx::query_as::<_, Note>(
        "SELECT id, notebook_id, title, content, is_trashed, created_at, updated_at \
         FROM notes WHERE is_trashed = 0 ORDER BY notebook_id, updated_at",
    )
    .fetch_all(pool)
    .await?;

    let planned = plan(&notebooks, &notes);
    tokio::fs::create_dir_all(out_dir).await?;
    for file in &planned {
        let mut path = out_dir.to_path_buf();
        for segment in &file.dir {
            path.push(segment);
        }
        tokio::fs::create_dir_all(&path).await?;
        path.push(&file.name);
        tokio::fs::write(&path, &file.content).await?;
    }
    Ok(planned.len())
}

use crate::core::repository::notebooks;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::NotebookId;

    fn nb(id: i64, parent: Option<i64>, name: &str) -> Notebook {
        Notebook {
            id: NotebookId(id),
            parent_id: parent.map(NotebookId),
            name: name.into(),
            created_at: "2026-01-01 00:00:00".into(),
            updated_at: "2026-01-01 00:00:00".into(),
        }
    }

    fn note(id: i64, notebook: Option<i64>, title: &str, content: &str) -> Note {
        Note {
            id: crate::core::model::NoteId(id),
            notebook_id: notebook.map(NotebookId),
            title: title.into(),
            content: content.into(),
            is_trashed: false,
            created_at: "2026-01-01 00:00:00".into(),
            updated_at: "2026-01-01 00:00:00".into(),
        }
    }

    #[test]
    fn sanitize_component_maps_unsafe_characters() {
        assert_eq!(sanitize_component("Hello World"), "Hello World");
        assert_eq!(sanitize_component("a/b\\c:d"), "a-b-c-d");
        assert_eq!(sanitize_component("  "), "untitled");
        assert_eq!(sanitize_component("///"), "untitled");
    }

    #[test]
    fn yaml_scalar_escapes_quotes_and_newlines() {
        assert_eq!(yaml_scalar("plain"), "\"plain\"");
        assert_eq!(yaml_scalar("with \"quotes\""), "\"with \\\"quotes\\\"\"");
        assert_eq!(yaml_scalar("line1\nline2"), "\"line1\\nline2\"");
        assert_eq!(yaml_scalar("back\\slash"), "\"back\\\\slash\"");
    }

    #[test]
    fn nested_notebooks_map_to_nested_directories() {
        let notebooks = vec![nb(1, None, "Work"), nb(2, Some(1), "2026")];
        let notes = vec![note(10, Some(2), "Deep", "body")];
        let plan = plan(&notebooks, &notes);
        assert_eq!(plan[0].dir, vec!["Work", "2026"]);
        assert_eq!(plan[0].name, "Deep.md");
    }

    #[test]
    fn same_title_in_different_directories_does_not_collide() {
        let notebooks = vec![nb(1, None, "A"), nb(2, None, "B")];
        let notes = vec![
            note(10, Some(1), "Todo", "a"),
            note(11, Some(2), "Todo", "b"),
        ];
        let plan = plan(&notebooks, &notes);
        assert_eq!(plan[0].name, "Todo.md");
        assert_eq!(plan[1].name, "Todo.md");
        assert_eq!(plan[0].dir, vec!["A"]);
        assert_eq!(plan[1].dir, vec!["B"]);
    }

    #[test]
    fn same_title_in_same_directory_gets_numeric_suffix() {
        let notebooks = vec![];
        let notes = vec![
            note(10, None, "Todo", "first"),
            note(11, None, "Todo", "second"),
        ];
        let plan = plan(&notebooks, &notes);
        assert_eq!(plan[0].name, "Todo.md");
        assert_eq!(plan[1].name, "Todo-1.md");
    }

    #[test]
    fn unfiled_notes_export_to_the_root() {
        let notes = vec![note(10, None, "Loose", "body")];
        let plan = plan(&[], &notes);
        assert!(plan[0].dir.is_empty());
    }
}
