//! Full-text search over the FTS5 index.

use sqlx::SqlitePool;

use crate::core::Result;
use crate::core::model::SearchHit;
use crate::core::search::fts_query;

/// Run a full-text search over title + content, newest first, capped at
/// 200 hits. An empty (after escaping) query returns no hits.
pub(crate) async fn run(pool: &SqlitePool, query: &str) -> Result<Vec<SearchHit>> {
    let match_expr = fts_query(query);
    if match_expr.trim().is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, SearchHit>(
        "SELECT n.id, n.title, \
                snippet(notes_fts, 1, '⟪', '⟫', '…', 14) AS snippet \
         FROM notes_fts \
         JOIN notes n ON n.id = notes_fts.rowid \
         WHERE notes_fts MATCH ? AND n.is_trashed = 0 \
         ORDER BY rank LIMIT 200",
    )
    .bind(match_expr)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
