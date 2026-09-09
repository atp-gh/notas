//! Notebook rows: tree operations and path resolution.

use std::collections::{HashMap, HashSet};

use sqlx::SqlitePool;

use crate::core::error::{Error, Result};
use crate::core::model::{Notebook, NotebookId};
use crate::core::repository::notes::ensure_affected;

/// Validate a notebook display name and return its trimmed form.
///
/// The name becomes a path segment in several places with different rules:
/// a `'/'` would read as a nesting separator during sync, and a `'\\'`
/// would collide with file separators on export (and is already rejected
/// in sync uuids). Rejecting both up front keeps the three
/// representations from silently diverging; empty names are rejected too.
/// The trimmed value is stored so names never carry accidental padding.
fn validate_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidInput(
            "notebook name must not be empty".into(),
        ));
    }
    if trimmed.contains(['/', '\\']) {
        return Err(Error::InvalidInput(
            "notebook name must not contain '/' or '\\'".into(),
        ));
    }
    Ok(trimmed.to_owned())
}

/// Turn a UNIQUE-constraint failure (a sibling notebook already carries
/// this name) into the typed duplicate-name error; any other failure
/// passes through unchanged.
fn map_duplicate(err: sqlx::Error, name: &str) -> Error {
    match err {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            Error::NotebookNameExists(name.to_owned())
        }
        other => other.into(),
    }
}

/// Build a conflict-free copy name: `base (copy)`, then `base (copy 2)`…
/// (English numerals per product decision).
///
/// Takes `&str` and borrows the existing-name set so callers never clone
/// the sibling list just to name one copy.
#[must_use]
pub(crate) fn unique_copy_name(base: &str, existing: &HashSet<String>) -> String {
    let first = format!("{base} (copy)");
    if !existing.contains(&first) {
        return first;
    }
    let mut n = 2_u32;
    loop {
        let candidate = format!("{base} (copy {n})");
        if !existing.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Insert a notebook and return the created row.
pub(crate) async fn create(
    pool: &SqlitePool,
    parent_id: Option<NotebookId>,
    name: &str,
) -> Result<Notebook> {
    let name = validate_name(name)?;
    let row = sqlx::query_as::<_, Notebook>(
        "INSERT INTO notebooks (parent_id, name) VALUES (?, ?) RETURNING \
         id, parent_id, name, is_trashed, created_at, updated_at",
    )
    .bind(parent_id)
    .bind(&name)
    .fetch_one(pool)
    .await
    .map_err(|err| map_duplicate(err, &name))?;
    Ok(row)
}

/// Rename a notebook and bump its `updated_at`.
pub(crate) async fn rename(pool: &SqlitePool, id: NotebookId, name: &str) -> Result<()> {
    let name = validate_name(name)?;
    let result =
        sqlx::query("UPDATE notebooks SET name = ?, updated_at = datetime('now') WHERE id = ?")
            .bind(&name)
            .bind(id)
            .execute(pool)
            .await
            .map_err(|err| map_duplicate(err, &name))?;
    ensure_affected(result.rows_affected(), || Error::NotebookNotFound(id))
}

/// Delete a notebook. Nested notebooks cascade; its notes become unfiled
/// (`notebook_id` is set to NULL by the schema).
pub(crate) async fn delete(pool: &SqlitePool, id: NotebookId) -> Result<()> {
    let result = sqlx::query("DELETE FROM notebooks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    ensure_affected(result.rows_affected(), || Error::NotebookNotFound(id))
}

/// All live notebooks, sorted by name (trashed rows stay in the trash view).
pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<Notebook>> {
    let rows = sqlx::query_as::<_, Notebook>(
        "SELECT id, parent_id, name, is_trashed, created_at, updated_at \
         FROM notebooks WHERE is_trashed = 0 ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// All trashed notebooks, sorted by name.
pub(crate) async fn list_trashed(pool: &SqlitePool) -> Result<Vec<Notebook>> {
    let rows = sqlx::query_as::<_, Notebook>(
        "SELECT id, parent_id, name, is_trashed, created_at, updated_at \
         FROM notebooks WHERE is_trashed = 1 ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Live sibling names under `parent` (trashed siblings excluded by the
/// partial unique index, so they never block a copy/move).
async fn sibling_names(
    executor: &mut sqlx::SqliteConnection,
    parent: Option<NotebookId>,
) -> Result<HashSet<String>> {
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT name FROM notebooks WHERE parent_id IS ? AND is_trashed = 0")
            .bind(parent)
            .fetch_all(&mut *executor)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Collect `root` plus every descendant id (root first, then BFS).
/// Reads the whole (small) table once instead of recursing in SQL.
async fn subtree_ids(
    executor: &mut sqlx::SqliteConnection,
    root: NotebookId,
) -> Result<Vec<NotebookId>> {
    let rows: Vec<(NotebookId, Option<NotebookId>)> =
        sqlx::query_as("SELECT id, parent_id FROM notebooks")
            .fetch_all(&mut *executor)
            .await?;
    let mut children: HashMap<Option<NotebookId>, Vec<NotebookId>> = HashMap::new();
    for (id, parent) in rows {
        children.entry(parent).or_default().push(id);
    }
    let mut ordered = vec![root];
    let mut index = 0;
    while index < ordered.len() {
        let current = ordered[index];
        index += 1;
        if let Some(next) = children.get(&Some(current)) {
            ordered.extend(next.iter().copied());
        }
    }
    Ok(ordered)
}

/// Walk from `id` up to the root, returning the ancestor chain root-first.
async fn ancestor_chain(
    executor: &mut sqlx::SqliteConnection,
    id: NotebookId,
) -> Result<Vec<NotebookId>> {
    let mut chain = Vec::new();
    let mut current = Some(id);
    // Depth guard: a corrupt parent cycle must not loop forever.
    for _ in 0..64 {
        let Some(node) = current else { break };
        chain.push(node);
        let parent: Option<NotebookId> =
            sqlx::query_scalar("SELECT parent_id FROM notebooks WHERE id = ?")
                .bind(node)
                .fetch_optional(&mut *executor)
                .await?
                .flatten()
                .flatten();
        current = parent;
    }
    chain.reverse();
    Ok(chain)
}

/// Move a notebook subtree under `target_parent` (`None` = top level).
///
/// Rejects moves into the subtree itself (which would cycle the tree) and
/// no-op moves; sibling name clashes are resolved with a `(copy)` suffix
/// instead of failing.
pub(crate) async fn move_to(
    pool: &SqlitePool,
    id: NotebookId,
    target_parent: Option<NotebookId>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let current: Option<Option<NotebookId>> =
        sqlx::query_scalar("SELECT parent_id FROM notebooks WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(current_parent) = current else {
        return Err(Error::NotebookNotFound(id));
    };
    if current_parent == target_parent {
        return Ok(());
    }
    if let Some(target) = target_parent {
        if target == id {
            return Err(Error::InvalidInput(
                "cannot move a notebook into itself".into(),
            ));
        }
        let subtree = subtree_ids(&mut tx, id).await?;
        if subtree.contains(&target) {
            return Err(Error::InvalidInput(
                "cannot move a notebook into its own subtree".into(),
            ));
        }
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT id FROM notebooks WHERE id = ? AND is_trashed = 0")
                .bind(target)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Err(Error::NotebookNotFound(target));
        }
    }
    // Resolve a sibling clash before the UPDATE so the partial unique
    // index never fires on a move.
    let mut siblings = sibling_names(&mut tx, target_parent).await?;
    let own_name: String = sqlx::query_scalar("SELECT name FROM notebooks WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    siblings.remove(&own_name);
    let name = if siblings.contains(&own_name) {
        unique_copy_name(&own_name, &siblings)
    } else {
        own_name
    };
    let result = sqlx::query(
        "UPDATE notebooks SET parent_id = ?, name = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(target_parent)
    .bind(&name)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    // The row was selected above, so zero affected rows means a
    // concurrent delete won the race: surface it instead of committing
    // a no-op move.
    ensure_affected(result.rows_affected(), || Error::NotebookNotFound(id))?;
    tx.commit().await?;
    Ok(())
}

/// Recursively trash a notebook subtree: flags the notebooks plus every
/// contained note (both bump `updated_at` so sync sees the change).
pub(crate) async fn trash_subtree(pool: &SqlitePool, id: NotebookId) -> Result<()> {
    let mut tx = pool.begin().await?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM notebooks WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(Error::NotebookNotFound(id));
    }
    let subtree = subtree_ids(&mut tx, id).await?;
    for node in &subtree {
        sqlx::query(
            "UPDATE notebooks SET is_trashed = 1, updated_at = datetime('now') WHERE id = ?",
        )
        .bind(*node)
        .execute(&mut *tx)
        .await?;
    }
    for node in &subtree {
        sqlx::query(
            "UPDATE notes SET is_trashed = 1, updated_at = datetime('now') \
             WHERE notebook_id = ? AND is_trashed = 0",
        )
        .bind(*node)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Recursively restore a trashed subtree. Ancestors are restored first so
/// the tree never dangles; sibling name clashes on the way back resolve
/// with a `(copy)` suffix instead of failing the restore.
pub(crate) async fn restore_subtree(pool: &SqlitePool, id: NotebookId) -> Result<()> {
    let mut tx = pool.begin().await?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM notebooks WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(Error::NotebookNotFound(id));
    }
    // Ancestors first (root-first), then the whole subtree below `id`.
    let mut to_restore = ancestor_chain(&mut tx, id).await?;
    let subtree = subtree_ids(&mut tx, id).await?;
    for node in subtree {
        if !to_restore.contains(&node) {
            to_restore.push(node);
        }
    }
    for node in &to_restore {
        let (parent, own): (Option<NotebookId>, String) =
            sqlx::query_as("SELECT parent_id, name FROM notebooks WHERE id = ?")
                .bind(*node)
                .fetch_one(&mut *tx)
                .await?;
        let mut siblings = sibling_names(&mut tx, parent).await?;
        siblings.remove(&own);
        let final_name = if siblings.contains(&own) {
            unique_copy_name(&own, &siblings)
        } else {
            own
        };
        sqlx::query(
            "UPDATE notebooks SET is_trashed = 0, name = ?, updated_at = datetime('now') \
             WHERE id = ?",
        )
        .bind(&final_name)
        .bind(*node)
        .execute(&mut *tx)
        .await?;
    }
    for node in &to_restore {
        sqlx::query(
            "UPDATE notes SET is_trashed = 0, updated_at = datetime('now') \
             WHERE notebook_id = ? AND is_trashed = 1",
        )
        .bind(*node)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Deep-copy a notebook subtree under `target_parent`, duplicating every
/// contained note (title + content + tags, fresh ids, `is_trashed = 0`).
/// Returns the new subtree root.
pub(crate) async fn duplicate_subtree(
    pool: &SqlitePool,
    id: NotebookId,
    target_parent: Option<NotebookId>,
) -> Result<Notebook> {
    // Cycle check first (same rule as move): a copy into its own subtree
    // would duplicate the tree under itself.
    if let Some(target) = target_parent {
        let mut conn = pool.acquire().await?;
        let subtree = subtree_ids(&mut conn, id).await?;
        if subtree.contains(&target) {
            return Err(Error::InvalidInput(
                "cannot copy a notebook into its own subtree".into(),
            ));
        }
    }
    let mut tx = pool.begin().await?;
    let source: Option<Notebook> = sqlx::query_as::<_, Notebook>(
        "SELECT id, parent_id, name, is_trashed, created_at, updated_at \
         FROM notebooks WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(source) = source else {
        return Err(Error::NotebookNotFound(id));
    };
    // Ordered root-first so parents are copied before their children.
    let subtree = subtree_ids(&mut tx, id).await?;
    let mut id_map: HashMap<NotebookId, NotebookId> = HashMap::new();
    let mut new_root: Option<Notebook> = None;
    // Sibling names per new parent, kept in memory so sequential copies
    // within one subtree cannot collide with each other.
    let mut sibling_cache: HashMap<Option<NotebookId>, HashSet<String>> = HashMap::new();
    for old_id in subtree {
        let (old_parent, old_name): (Option<NotebookId>, String) =
            sqlx::query_as("SELECT parent_id, name FROM notebooks WHERE id = ?")
                .bind(old_id)
                .fetch_one(&mut *tx)
                .await?;
        let new_parent = if old_id == id {
            target_parent
        } else {
            old_parent.and_then(|p| id_map.get(&p).copied())
        };
        let siblings = match sibling_cache.get(&new_parent) {
            Some(set) => set.clone(),
            None => sibling_names(&mut tx, new_parent).await?,
        };
        let name = unique_copy_name(&old_name, &siblings);
        let new_row: Notebook = sqlx::query_as::<_, Notebook>(
            "INSERT INTO notebooks (parent_id, name) VALUES (?, ?) RETURNING \
             id, parent_id, name, is_trashed, created_at, updated_at",
        )
        .bind(new_parent)
        .bind(&name)
        .fetch_one(&mut *tx)
        .await?;
        let mut updated = siblings;
        updated.insert(name);
        sibling_cache.insert(new_parent, updated);
        id_map.insert(old_id, new_row.id);
        if old_id == id {
            new_root = Some(new_row);
        }
    }
    let Some(new_root) = new_root else {
        return Err(Error::NotebookNotFound(source.id));
    };
    // Copy notes: same title-suffix rule as single-note duplicates, but
    // scoped per new notebook so two pastes never share one "(copy)" name.
    // One query per notebook keeps the SQL trivial (no dynamic IN list)
    // and the whole copy stays in the same transaction.
    let mut title_cache: HashMap<NotebookId, HashSet<String>> = HashMap::new();
    // Collect first so the tx borrow ends before the insert loop re-borrows.
    let mut pending_notes: Vec<(NotebookId, String, String, Vec<String>)> = Vec::new();
    for (old_id, new_id) in &id_map {
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, title, content FROM notes WHERE notebook_id = ?")
                .bind(*old_id)
                .fetch_all(&mut *tx)
                .await?;
        for (note_id, title, content) in rows {
            let tag_names: Vec<String> = sqlx::query_scalar(
                "SELECT t.name FROM tags t JOIN note_tags nt ON nt.tag_id = t.id \
                 WHERE nt.note_id = ? ORDER BY t.name",
            )
            .bind(note_id)
            .fetch_all(&mut *tx)
            .await?;
            pending_notes.push((*new_id, title, content, tag_names));
        }
    }
    for (new_nb, title, content, tag_names) in pending_notes {
        let titles = match title_cache.get(&new_nb) {
            Some(set) => set.clone(),
            None => {
                let rows: Vec<String> =
                    sqlx::query_scalar("SELECT title FROM notes WHERE notebook_id = ?")
                        .bind(new_nb)
                        .fetch_all(&mut *tx)
                        .await?;
                rows.into_iter().collect()
            }
        };
        let new_title = unique_copy_name(&title, &titles);
        let new_note_id: crate::core::model::NoteId = sqlx::query_scalar(
            "INSERT INTO notes (notebook_id, title, content, is_trashed) \
             VALUES (?, ?, ?, 0) RETURNING id",
        )
        .bind(new_nb)
        .bind(&new_title)
        .bind(&content)
        .fetch_one(&mut *tx)
        .await?;
        crate::core::repository::tags::replace_note_tags(&mut tx, new_note_id, &tag_names).await?;
        let mut updated = titles;
        updated.insert(new_title);
        title_cache.insert(new_nb, updated);
    }
    tx.commit().await?;
    // Re-read the new root so the returned row reflects committed state.
    let root = sqlx::query_as::<_, Notebook>(
        "SELECT id, parent_id, name, is_trashed, created_at, updated_at \
         FROM notebooks WHERE id = ?",
    )
    .bind(new_root.id)
    .fetch_one(pool)
    .await?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_name_uses_plain_copy_suffix_first() {
        let existing: HashSet<String> = HashSet::new();
        assert_eq!(unique_copy_name("Report", &existing), "Report (copy)");
    }

    #[test]
    fn copy_name_numbers_repeated_copies() {
        let existing: HashSet<String> = ["Report (copy)".into()].into_iter().collect();
        assert_eq!(unique_copy_name("Report", &existing), "Report (copy 2)");
    }

    #[test]
    fn copy_name_skips_taken_numbers() {
        let existing: HashSet<String> = ["X (copy)".into(), "X (copy 2)".into()]
            .into_iter()
            .collect();
        assert_eq!(unique_copy_name("X", &existing), "X (copy 3)");
    }
}
