//! Markdown import: a Joplin-export parser plus a transactional database
//! writer.
//!
//! [`parse_front_matter`] reads the YAML block Joplin's "Export all as
//! Markdown + Front Matter" writes at the top of every note
//! (`title`/`created`/`updated`/`tags`/`id`); [`preview`] walks an export
//! directory and counts what an import would touch (for the confirmation
//! dialog), and [`run`] recreates the notebook tree and upserts every note
//! inside one transaction. The mirror image of `super::export`: same
//! directory layout (notebooks as folders, notes as `.md` files), same
//! hand-rolled YAML handling — no new dependencies.
//!
//! Exports without front matter (Joplin's older "MD - Markdown" option)
//! are accepted too: the file name becomes the title. Directories named
//! `_resources` (Joplin's attachment folder) and non-`.md` files are
//! skipped; empty directories still become empty notebooks.

use std::collections::HashMap;
use std::path::Path;

use sqlx::SqlitePool;

use crate::core::error::Result;
use crate::core::model::{NoteId, NotebookId};
use crate::core::repository::tags::replace_note_tags;

/// Guard against pathological directory depth, matching the export planner
/// and the sync-index path walker.
const MAX_DEPTH: usize = 64;

/// Preview counts shown in the confirmation dialog. Computed by walking
/// the tree only; no file contents are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportPreview {
    /// Directories that will become notebooks (excluding `_resources`).
    pub notebooks: usize,
    /// `.md` files that will become notes.
    pub notes: usize,
}

/// Outcome of one import run, reported to the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportStats {
    /// Notebook levels created during this run.
    pub notebooks_created: usize,
    /// Notebook levels that already existed and were reused (merged).
    pub notebooks_found: usize,
    /// Notes inserted as new rows.
    pub notes_imported: usize,
    /// Existing notes (matched by their Joplin id) overwritten in place.
    pub notes_updated: usize,
    /// `.md` files that could not be read (permissions, invalid UTF-8).
    pub notes_skipped: usize,
}

/// One planned note: everything needed to upsert it into the database.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedNote {
    /// Display title (front matter `title`, else the file stem).
    title: String,
    /// Markdown body, without the front matter block.
    content: String,
    /// Tag names from the front matter.
    tags: Vec<String>,
    /// Front matter `created`, in Joplin's ISO 8601 form (or the plain
    /// `YYYY-MM-DD HH:MM:SS` form Notas exports); [`to_sqlite_datetime`]
    /// normalizes it to the SQLite form before it is bound.
    created: Option<String>,
    /// Front matter `updated`, same shape as `created`.
    updated: Option<String>,
    /// Front matter `id` (Joplin's 32-hex note id), stored as the sync
    /// uuid so a re-import updates instead of duplicating.
    joplin_id: Option<String>,
}

/// One `.md` file discovered under the export root.
struct FoundFile {
    /// Sanitized notebook path segments, `None` for files in the root.
    notebook: Option<Vec<String>>,
    /// Absolute path of the file.
    path: std::path::PathBuf,
    /// File name without the `.md` extension (the title fallback).
    stem: String,
}

/// The subset of Joplin's front matter the import understands. Every other
/// key (`source`, `source_url`, geo fields, …) is ignored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrontMatter {
    /// `title:` — overrides the file name when present.
    pub title: Option<String>,
    /// `created:` — ISO 8601 (possibly with timezone offset).
    pub created: Option<String>,
    /// `updated:` — same shape as `created`.
    pub updated: Option<String>,
    /// `id:` — Joplin's stable note id, reused as the sync uuid.
    pub id: Option<String>,
    /// `tags:` — a flow list (`[a, b]`) or block list (`- a`), unquoted.
    pub tags: Vec<String>,
}

/// Parse Joplin/Notas-style YAML front matter: a leading `---` line, a
/// closing `---` line, and the note body after it.
///
/// Returns `None` when the content has no front matter block (the whole
/// file is the body) or when the closing fence is missing.
#[must_use]
pub fn parse_front_matter(content: &str) -> Option<(FrontMatter, &str)> {
    let first = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    let mut fm = FrontMatter::default();
    let mut tags_block = false;
    let mut cursor = 0usize;
    for line in first.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if text == "---" {
            return Some((fm, &first[cursor + line.len()..]));
        }
        if tags_block {
            if let Some(item) = text.trim_start().strip_prefix('-').map(str::trim)
                && !item.is_empty()
            {
                fm.tags.push(unquote(item));
                cursor += line.len();
                continue;
            }
            tags_block = false;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            cursor += line.len();
            continue;
        }
        if let Some((key, value)) = text.split_once(':') {
            match key.trim().to_ascii_lowercase().as_str() {
                "title" => fm.title = maybe_quoted(value),
                "created" => fm.created = maybe_quoted(value),
                "updated" => fm.updated = maybe_quoted(value),
                "id" => fm.id = maybe_quoted(value),
                "tags" => {
                    let value = value.trim();
                    if value.starts_with('[') {
                        fm.tags = flow_list(value);
                    } else if value.is_empty() {
                        tags_block = true;
                    } else {
                        fm.tags = vec![unquote(value)];
                    }
                }
                _ => {}
            }
        }
        cursor += line.len();
    }
    None
}

/// Decode one scalar value: strip the surrounding quotes (unescaping
/// `\"` and `\\` inside), or return the trimmed plain text.
fn scalar(value: &str) -> Option<String> {
    let value = value.trim();
    let rest = value.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            '"' => break,
            c => out.push(c),
        }
    }
    Some(out)
}

/// Decode a scalar that may be quoted (see [`scalar`]) or plain.
fn maybe_quoted(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(scalar(value).unwrap_or_else(|| value.to_string()))
    }
}

/// Decode a tag item: unquote when double- or single-quoted.
fn unquote(item: &str) -> String {
    let item = item.trim();
    if item.starts_with('"') {
        scalar(item).unwrap_or_default()
    } else if let Some(inner) = item.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        inner.to_string()
    } else {
        item.to_string()
    }
}

/// Parse a YAML flow list (`[a, "b, c", d]`), honoring quotes so commas
/// inside quoted items do not split the list.
fn flow_list(value: &str) -> Vec<String> {
    let inner = value
        .trim()
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(value.trim());
    let mut items = Vec::new();
    let mut start = 0usize;
    let mut in_quote = false;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_quote => escaped = true,
            '"' if in_quote => in_quote = false,
            '"' => in_quote = true,
            ',' if !in_quote => {
                items.push(unquote(&inner[start..i]));
                start = i + 1;
            }
            _ => {}
        }
    }
    items.push(unquote(&inner[start..]));
    items.retain(|item| !item.is_empty());
    items
}

/// Convert a front-matter timestamp to SQLite's `YYYY-MM-DD HH:MM:SS`
/// (UTC) form, so the stored value compares and sorts like every other
/// Notas timestamp.
///
/// Accepts what Joplin and Notas emit: the plain `YYYY-MM-DD HH:MM:SS`
/// form (passed through, any timezone offset applied) and ISO 8601
/// `YYYY-MM-DDTHH:MM:SS[.fff][Z|±HH:MM]` (normalized to UTC, fractional
/// seconds dropped). Returns `None` for anything else; the caller falls
/// back to the current time.
///
/// The conversion is deliberate and local (no SQL `datetime()`): relying
/// on `datetime(?)` inside the note INSERT silently stored the raw value
/// when the same transaction had already executed other statements (a
/// sqlx/SQLite interaction observed while testing), so the import must
/// not depend on it.
#[must_use]
pub fn to_sqlite_datetime(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() < 19 {
        return None;
    }
    if !matches!(value.as_bytes().get(10), Some(b' ' | b'T')) {
        return None;
    }
    let (year, month, day) = parse_ymd(&value[..10])?;
    let (hour, minute, second, offset) = parse_time_with_zone(&value[11..])?;
    if !is_valid_day(year, month, day) {
        return None;
    }
    // Total seconds since 1970-01-01 UTC; subtract the zone offset so the
    // instant is preserved when the front matter carried one.
    let total = days_from_civil(i64::from(year), month, day) * 86_400
        + i64::from(hour) * 3_600
        + i64::from(minute) * 60
        + i64::from(second)
        - offset;
    let days = total.div_euclid(86_400);
    let seconds = total.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Some(format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds % 3_600) / 60,
        seconds % 60
    ))
}

/// Parse a fixed-length `YYYY-MM-DD` prefix into its three components.
fn parse_ymd(value: &str) -> Option<(u32, u32, u32)> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let year: u32 = value[..4].parse().ok()?;
    let month: u32 = value[5..7].parse().ok()?;
    let day: u32 = value[8..10].parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some((year, month, day))
}

/// Parse `HH:MM:SS[.fff][Z|±HH:MM]` into the clock time and the timezone
/// offset in seconds east of UTC (0 when absent). A `:60` second (leap
/// second) is tolerated, mirroring the sync planner.
fn parse_time_with_zone(value: &str) -> Option<(u32, u32, u32, i64)> {
    let bytes = value.as_bytes();
    if bytes.len() < 8
        || bytes[2] != b':'
        || bytes[5] != b':'
        || !bytes[..2].iter().all(u8::is_ascii_digit)
        || !bytes[3..5].iter().all(u8::is_ascii_digit)
        || !bytes[6..8].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let hour: u32 = value[..2].parse().ok()?;
    let minute: u32 = value[3..5].parse().ok()?;
    let second: u32 = value[6..8].parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &value[8..];
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = &after_dot[digits..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => parse_offset(rest)?,
    };
    Some((hour, minute, second, offset))
}

/// Parse a `±HH:MM` timezone suffix into seconds east of UTC.
fn parse_offset(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 6
        || (bytes[0] != b'+' && bytes[0] != b'-')
        || bytes[3] != b':'
        || !bytes[1..3].iter().all(u8::is_ascii_digit)
        || !bytes[4..6].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let hours: i64 = value[1..3].parse().ok()?;
    let minutes: i64 = value[4..6].parse().ok()?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    let sign = if bytes[0] == b'-' { -1 } else { 1 };
    Some(sign * (hours * 3_600 + minutes * 60))
}

/// Reject impossible calendar dates (e.g. February 30) that the round-trip
/// through the civil-date algorithms would silently normalize.
fn is_valid_day(year: u32, month: u32, day: u32) -> bool {
    let (y, m, d) = civil_from_days(days_from_civil(i64::from(year), month, day));
    (y, m, d) == (i64::from(year), month, day)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (i64::from(if month > 2 { month - 3 } else { month + 9 })) + 2) / 5
        + i64::from(day)
        - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// Strip the blank lines that conventionally separate a front matter block
/// from the body (both Joplin and Notas exports write one). Only lines
/// that are entirely whitespace are removed; the first content line keeps
/// its indentation and everything after it is preserved verbatim.
fn strip_leading_blank_lines(body: &str) -> &str {
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if line.trim().is_empty() {
            offset += line.len();
        } else {
            break;
        }
    }
    &body[offset..]
}

/// Map a directory name to a notebook name: trim, replace the backslash
/// Notas rejects in notebook names, and fall back to `untitled`. Unicode
/// names (CJK included) pass through untouched — unlike the export
/// sanitizer, which is deliberately not reused here.
fn notebook_name(dir_name: &str) -> String {
    let name = dir_name.trim().replace('\\', "-");
    if name.is_empty() {
        "untitled".to_string()
    } else {
        name
    }
}

/// Walk the export tree, collecting every notebook path and `.md` file.
/// `_resources` subtrees, symlinks and non-`.md` files are ignored. An
/// explicit stack (instead of recursion) keeps the future size bounded.
async fn collect(root: &Path) -> Result<(Vec<Vec<String>>, Vec<FoundFile>)> {
    let meta = tokio::fs::metadata(root).await?;
    if !meta.is_dir() {
        return Err(crate::core::error::Error::InvalidInput(
            "import source is not a directory".into(),
        ));
    }
    let mut notebooks = Vec::new();
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), Vec::new())];
    while let Some((dir, segments)) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if file_type.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                if name == "_resources" {
                    continue;
                }
                let mut child_segments = segments.clone();
                child_segments.push(notebook_name(&name));
                notebooks.push(child_segments.clone());
                if child_segments.len() < MAX_DEPTH {
                    stack.push((entry.path(), child_segments));
                }
            } else if file_type.is_file() && entry.path().extension().is_some_and(|e| e == "md") {
                let stem = entry
                    .path()
                    .file_stem()
                    .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
                files.push(FoundFile {
                    notebook: (!segments.is_empty()).then(|| segments.clone()),
                    path: entry.path(),
                    stem,
                });
            }
        }
    }
    Ok((notebooks, files))
}

/// Resolve one notebook path segment-by-segment, creating missing levels
/// and counting created vs. found levels, inside the caller's transaction.
///
/// `resolved` memoizes every level touched this run (created or found) so
/// a shared ancestor is counted once and note lookups never re-count.
async fn resolve_notebook_path(
    tx: &mut sqlx::SqliteConnection,
    segments: &[String],
    resolved: &mut HashMap<String, NotebookId>,
    stats: &mut ImportStats,
) -> Result<Option<NotebookId>> {
    if segments.is_empty() {
        return Ok(None);
    }
    let mut parent: Option<NotebookId> = None;
    let mut key = String::new();
    for segment in segments {
        key.push('/');
        key.push_str(segment);
        if let Some(id) = resolved.get(&key) {
            parent = Some(*id);
            continue;
        }
        let found: Option<NotebookId> =
            sqlx::query_scalar("SELECT id FROM notebooks WHERE parent_id IS ? AND name = ?")
                .bind(parent)
                .bind(segment)
                .fetch_optional(&mut *tx)
                .await?;
        let id = match found {
            Some(id) => {
                stats.notebooks_found += 1;
                id
            }
            None => {
                let id = sqlx::query_scalar::<_, NotebookId>(
                    "INSERT INTO notebooks (parent_id, name) VALUES (?, ?) RETURNING id",
                )
                .bind(parent)
                .bind(segment)
                .fetch_one(&mut *tx)
                .await?;
                stats.notebooks_created += 1;
                id
            }
        };
        resolved.insert(key.clone(), id);
        parent = Some(id);
    }
    Ok(parent)
}

/// Upsert one planned note: insert it, or update the note carrying the
/// same Joplin id when one exists. Tags are replaced on either path.
///
/// `created_at`/`updated_at` arrive already normalized (see
/// [`to_sqlite_datetime`]) or `None`; the caller's `now` string fills in
/// the missing values on insert, while an update keeps an existing
/// `created_at` (a re-import without a `created` field must not clobber
/// the original creation time).
async fn upsert_note(
    tx: &mut sqlx::SqliteConnection,
    notebook_id: Option<NotebookId>,
    note: &PlannedNote,
    now: &str,
    stats: &mut ImportStats,
) -> Result<()> {
    let existing: Option<NoteId> = match &note.joplin_id {
        Some(id) => {
            sqlx::query_scalar("SELECT id FROM notes WHERE uuid = ?")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
        }
        None => None,
    };
    // Raw front-matter timestamps arrive in Joplin's/Notas' shapes; the
    // conversion must happen here, not in SQL (see [`to_sqlite_datetime`]).
    let created_at = note.created.as_deref().and_then(to_sqlite_datetime);
    let updated_at = note
        .updated
        .as_deref()
        .and_then(to_sqlite_datetime)
        .unwrap_or_else(|| now.to_string());
    let note_id = match existing {
        Some(id) => {
            sqlx::query(
                "UPDATE notes SET notebook_id = ?, title = ?, content = ?, is_trashed = 0, \
                 created_at = COALESCE(?, created_at), updated_at = ? WHERE id = ?",
            )
            .bind(notebook_id)
            .bind(&note.title)
            .bind(&note.content)
            .bind(created_at.as_deref())
            .bind(updated_at.as_str())
            .bind(id)
            .execute(&mut *tx)
            .await?;
            stats.notes_updated += 1;
            id
        }
        None => {
            let id = sqlx::query_scalar::<_, NoteId>(
                "INSERT INTO notes (uuid, notebook_id, title, content, is_trashed, \
                 created_at, updated_at) \
                 VALUES (?, ?, ?, ?, 0, ?, ?) RETURNING id",
            )
            .bind(note.joplin_id.as_deref())
            .bind(notebook_id)
            .bind(&note.title)
            .bind(&note.content)
            .bind(created_at.unwrap_or_else(|| now.to_string()))
            .bind(updated_at.as_str())
            .fetch_one(&mut *tx)
            .await?;
            stats.notes_imported += 1;
            id
        }
    };
    replace_note_tags(&mut *tx, note_id, &note.tags).await?;
    Ok(())
}

/// Count what an import of `root` would touch: notebook directories and
/// `.md` files, without reading any file contents.
///
/// # Errors
///
/// Returns [`crate::core::error::Error::Io`] when the tree cannot be
/// walked, or [`crate::core::error::Error::InvalidInput`] when `root` is
/// not a directory.
pub(crate) async fn preview(root: &Path) -> Result<ImportPreview> {
    let (notebooks, files) = collect(root).await?;
    Ok(ImportPreview {
        notebooks: notebooks.len(),
        notes: files.len(),
    })
}

/// Import every `.md` file under `root` into the database.
///
/// Notebook directories are recreated (existing same-name notebooks are
/// reused and counted as found), notes are inserted — or updated in place
/// when their Joplin `id` already exists — and the whole run is one
/// transaction: a database error rolls everything back. Files that cannot
/// be read are counted as skipped and do not abort the import.
///
/// # Errors
///
/// Returns [`crate::core::error::Error::Io`] when the tree cannot be
/// walked or a note cannot be read (skipped instead), or
/// [`crate::core::error::Error::Database`] when a write fails (rolling
/// the transaction back).
pub(crate) async fn run(pool: &SqlitePool, root: &Path) -> Result<ImportStats> {
    let (notebooks, files) = collect(root).await?;
    let mut stats = ImportStats::default();
    let mut tx = pool.begin().await?;
    // Import fallback timestamp, in SQLite's own format, so notes without
    // a parseable front-matter timestamp still sort consistently.
    let now: String = sqlx::query_scalar("SELECT datetime('now')")
        .fetch_one(&mut *tx)
        .await?;
    let mut resolved: HashMap<String, NotebookId> = HashMap::new();
    for segments in &notebooks {
        resolve_notebook_path(&mut tx, segments, &mut resolved, &mut stats).await?;
    }
    for file in &files {
        let content = match tokio::fs::read_to_string(&file.path).await {
            Ok(content) => content,
            Err(_) => {
                stats.notes_skipped += 1;
                continue;
            }
        };
        let (meta, body) = parse_front_matter(&content)
            .unwrap_or_else(|| (FrontMatter::default(), content.as_str()));
        let body = strip_leading_blank_lines(body);
        let title = meta
            .title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| {
                if file.stem.is_empty() {
                    "untitled".to_string()
                } else {
                    file.stem.clone()
                }
            });
        let notebook_id = match &file.notebook {
            Some(segments) => {
                resolve_notebook_path(&mut tx, segments, &mut resolved, &mut stats).await?
            }
            None => None,
        };
        upsert_note(
            &mut tx,
            notebook_id,
            &PlannedNote {
                title,
                content: body.to_string(),
                tags: meta.tags,
                created: meta.created,
                updated: meta.updated,
                joplin_id: meta.id.filter(|id| !id.trim().is_empty()),
            },
            &now,
            &mut stats,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> (FrontMatter, String) {
        let (fm, body) = parse_front_matter(content).expect("front matter expected");
        (fm, body.to_string())
    }

    #[test]
    fn plain_content_without_fences_has_no_front_matter() {
        assert!(parse_front_matter("just a body").is_none());
        assert!(parse_front_matter("").is_none());
        assert!(parse_front_matter("---\nno closing fence\nbody").is_none());
    }

    #[test]
    fn quoted_title_keeps_colons_quotes_and_backslashes() {
        let (fm, body) = parse(
            "---\ntitle: \"Note: with \\\"quotes\\\" and\\\\slash\"\ncreated: 2024-01-01T10:00:00.000Z\n---\n\nbody here",
        );
        assert_eq!(
            fm.title.as_deref(),
            Some("Note: with \"quotes\" and\\slash")
        );
        assert_eq!(fm.created.as_deref(), Some("2024-01-01T10:00:00.000Z"));
        assert_eq!(body, "\nbody here");
    }

    #[test]
    fn plain_title_and_id_are_extracted() {
        let (fm, body) =
            parse("---\nid: 0f8f4d5e6a7b8c9d0e1f2a3b4c5d6e7f\ntitle: My note\n---\nbody");
        assert_eq!(fm.id.as_deref(), Some("0f8f4d5e6a7b8c9d0e1f2a3b4c5d6e7f"));
        assert_eq!(fm.title.as_deref(), Some("My note"));
        assert_eq!(body, "body");
    }

    #[test]
    fn flow_tags_split_on_commas_outside_quotes() {
        let (fm, _) = parse("---\ntags: [alpha, \"beta, gamma\", \"delta\"]\n---\n");
        assert_eq!(fm.tags, vec!["alpha", "beta, gamma", "delta"]);
    }

    #[test]
    fn block_tags_collect_until_next_key() {
        let (fm, _) = parse("---\ntags:\n  - one\n  - two\nsource: joplin\n---\n");
        assert_eq!(fm.tags, vec!["one", "two"]);
    }

    #[test]
    fn single_tag_without_brackets_is_one_item() {
        let (fm, _) = parse("---\ntags: solo\n---\n");
        assert_eq!(fm.tags, vec!["solo"]);
    }

    #[test]
    fn empty_tags_forms_yield_no_tags() {
        let (fm, _) = parse("---\ntags: []\n---\n");
        assert!(fm.tags.is_empty());
        let (fm, _) = parse("---\ntags:\n---\nbody");
        assert!(fm.tags.is_empty());
        assert_eq!(fm, FrontMatter::default());
    }

    #[test]
    fn keys_with_leading_whitespace_are_recognized() {
        let (fm, _) = parse("---\n  title: padded\n---\n");
        assert_eq!(fm.title.as_deref(), Some("padded"));
    }

    #[test]
    fn notas_export_front_matter_round_trips() {
        let (fm, body) = parse(
            "---\ntitle: \"Deep note\"\ncreated: \"2026-01-01 00:00:00\"\nupdated: \"2026-01-02 03:04:05\"\n---\n\ncontent",
        );
        assert_eq!(fm.title.as_deref(), Some("Deep note"));
        assert_eq!(fm.created.as_deref(), Some("2026-01-01 00:00:00"));
        assert_eq!(fm.updated.as_deref(), Some("2026-01-02 03:04:05"));
        assert_eq!(body, "\ncontent");
    }

    #[test]
    fn crlf_front_matter_is_accepted() {
        let (fm, body) = parse("---\r\ntitle: \"CRLF note\"\r\n---\r\nbody");
        assert_eq!(fm.title.as_deref(), Some("CRLF note"));
        assert_eq!(body, "body");
    }

    #[test]
    fn unknown_keys_and_comments_are_ignored() {
        let (fm, _) =
            parse("---\n# a comment\nsource: joplin\nsource_url: https://example.com\n---\n");
        assert_eq!(fm, FrontMatter::default());
    }

    #[test]
    fn to_sqlite_datetime_passes_plain_form_through() {
        assert_eq!(
            to_sqlite_datetime("2024-01-01 10:00:00"),
            Some("2024-01-01 10:00:00".to_string())
        );
    }

    #[test]
    fn to_sqlite_datetime_normalizes_iso_utc() {
        assert_eq!(
            to_sqlite_datetime("2024-01-01T10:00:00.000Z"),
            Some("2024-01-01 10:00:00".to_string())
        );
        assert_eq!(
            to_sqlite_datetime("2024-06-15T08:30:00Z"),
            Some("2024-06-15 08:30:00".to_string())
        );
        // No zone suffix means UTC.
        assert_eq!(
            to_sqlite_datetime("2024-01-01T10:00:00"),
            Some("2024-01-01 10:00:00".to_string())
        );
    }

    #[test]
    fn to_sqlite_datetime_applies_offset_to_plain_form() {
        assert_eq!(
            to_sqlite_datetime("2024-01-01 18:00:00+08:00"),
            Some("2024-01-01 10:00:00".to_string())
        );
    }

    #[test]
    fn to_sqlite_datetime_applies_timezone_offsets() {
        assert_eq!(
            to_sqlite_datetime("2024-01-01T18:00:00.000+08:00"),
            Some("2024-01-01 10:00:00".to_string())
        );
        // Negative offset pushes the instant later in the day.
        assert_eq!(
            to_sqlite_datetime("2024-01-01T02:00:00.000-05:00"),
            Some("2024-01-01 07:00:00".to_string())
        );
        // Offset rolls the calendar day over.
        assert_eq!(
            to_sqlite_datetime("2024-01-01T01:00:00.000+14:00"),
            Some("2023-12-31 11:00:00".to_string())
        );
    }

    #[test]
    fn to_sqlite_datetime_tolerates_leap_seconds() {
        assert_eq!(
            to_sqlite_datetime("2024-01-01T10:00:60Z"),
            Some("2024-01-01 10:01:00".to_string())
        );
    }

    #[test]
    fn to_sqlite_datetime_rejects_invalid_input() {
        for bad in [
            "",
            "garbage",
            "2024-01-01",
            "2024-01-01T10:00",
            "2024-13-01T10:00:00Z",
            "2024-01-01T25:00:00Z",
            "2024-02-30T10:00:00Z",
            "2024-01-01T10:00:00+8:00",
        ] {
            assert_eq!(to_sqlite_datetime(bad), None, "{bad}");
        }
    }

    #[test]
    fn strip_leading_blank_lines_only_removes_blank_lines() {
        assert_eq!(strip_leading_blank_lines("body"), "body");
        assert_eq!(strip_leading_blank_lines("\n\nbody"), "body");
        assert_eq!(strip_leading_blank_lines("\r\n\r\nbody"), "body");
        // Indented content keeps its indentation.
        assert_eq!(strip_leading_blank_lines("\n    - item\n"), "    - item\n");
        // Trailing newlines are untouched.
        assert_eq!(strip_leading_blank_lines("\nbody\n"), "body\n");
    }

    #[test]
    fn notebook_name_preserves_unicode_and_maps_backslashes() {
        assert_eq!(notebook_name("工作笔记"), "工作笔记");
        assert_eq!(notebook_name("  spaced  "), "spaced");
        assert_eq!(notebook_name("a\\b"), "a-b");
        assert_eq!(notebook_name("   "), "untitled");
    }
}
