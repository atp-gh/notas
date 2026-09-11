//! Attachments (Joplin-style resources).
//!
//! A resource is a globally-addressed binary blob (image, PDF, …) that one
//! or more notes reference from their Markdown body with Joplin's internal
//! link form `![alt](:/<id>)` / `[alt](:/<id>)`. Resources are shared: copying
//! a note reuses the same id without copying bytes, and deleting a note only
//! orphans its resources — the sync engine garbage-collects unreferenced
//! blobs after convergence (see `crate::core::sync`).
//!
//! This module is pure (no I/O, no SQL): id validation, the 100 MiB size cap,
//! Markdown link scanning/rewriting, export filename mapping
//! (`<id>-<original>` for lossless round-trips with a Joplin-compatible
//! fallback), and MIME guessing. Storage (SQLite rows + `resources/` files)
//! lives in `super::repository::resources`; transport in `crate::sync`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Hard cap for a single attachment: 100 MiB. Enforced on add/import; sync
/// never fragments, so anything larger would also risk backend timeouts.
pub const MAX_ATTACHMENT_BYTES: u64 = 100 * 1024 * 1024;

/// Length of a resource id in hex characters (`randomblob(16)` → 32 hex).
const RESOURCE_ID_LEN: usize = 32;

/// Whether `id` is usable as a resource address: 32 ASCII hex chars
/// (Joplin ids and Notas `lower(hex(randomblob(16)))` alike).
#[must_use]
pub fn is_valid_resource_id(id: &str) -> bool {
    id.len() == RESOURCE_ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Generate a fresh resource id (32 lowercase hex chars).
#[must_use]
pub fn new_resource_id() -> String {
    use rand::TryRngCore;
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OS RNG must be available for resource ids");
    hex::encode(bytes)
}

/// Split an export filename of the form `<id>-<original>` into its parts.
/// Returns `None` when there is no valid id prefix (a plain Joplin export
/// like `picture.png`, which imports under a fresh id).
#[must_use]
pub fn split_id_prefix(filename: &str) -> Option<(&str, &str)> {
    let (head, tail) = filename.split_once('-')?;
    if is_valid_resource_id(head) && !tail.is_empty() {
        Some((head, tail))
    } else {
        None
    }
}

/// Export filename for a resource: `<id>-<sanitized original>`. The id
/// prefix makes Notas→Notas re-imports lossless while staying readable for
/// Joplin (which treats the whole name as opaque).
#[must_use]
pub fn export_filename(id: &str, original: &str) -> String {
    let safe = sanitize_filename(original);
    format!("{id}-{safe}")
}

/// Keep a user filename safe for a single path component: strip directory
/// separators and control characters, trim, fall back to `file`. Unicode
/// (CJK included) passes through — unlike the note-title sanitizer, which
/// is ASCII-portable by design.
#[must_use]
pub fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c == '/' || c == '\\' || c.is_control() {
            out.push('-');
        } else {
            out.push(c);
        }
    }
    let trimmed = out.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        return "file".to_string();
    }
    // Bound the component so `<id>-<name>` stays well under PATH_MAX.
    const MAX_LEN: usize = 180;
    if trimmed.len() > MAX_LEN {
        // Truncate on a char boundary, keeping the extension when present.
        let mut end = MAX_LEN;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        trimmed[..end].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Guess a MIME type from the filename extension. Used for display/open
/// hints only — never for security decisions.
#[must_use]
pub fn guess_mime(filename: &str) -> String {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "txt" | "md" | "markdown" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Whether a MIME type should render inline as an image.
#[must_use]
pub fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/")
}

/// Directory holding local resource blobs: `<data_dir>/resources`.
#[must_use]
pub fn resources_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("resources")
}

/// Path of one local blob by resource id.
#[must_use]
pub fn resource_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(id)
}

/// Scan Markdown for Joplin resource references (`](:/<id>)`, with an
/// optional `"title"` suffix) and return the referenced ids in order
/// (duplicates kept — callers dedupe when counting).
#[must_use]
pub fn extract_resource_ids(content: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i + 5 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b'(' && bytes[i + 2] == b':' && bytes[i + 3] == b'/'
        {
            let start = i + 4;
            let mut end = start;
            while end < bytes.len()
                && !matches!(bytes[end], b')' | b'"' | b' ' | b'\t' | b'\n' | b'\r')
            {
                end += 1;
            }
            if end > start
                && let Ok(id) = std::str::from_utf8(&bytes[start..end])
                && !id.is_empty()
                && !id.contains('/')
                && !id.contains('\\')
            {
                ids.push(id.to_string());
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    ids
}

/// Collect every id referenced by any of the given note bodies.
#[must_use]
pub fn referenced_ids<'a>(contents: impl IntoIterator<Item = &'a str>) -> HashSet<String> {
    let mut set = HashSet::new();
    for content in contents {
        for id in extract_resource_ids(content) {
            set.insert(id);
        }
    }
    set
}

/// Rewrite internal `:/<id>` destinations to export-relative
/// `_resources/` paths. `depth` is the note file's directory depth (0 for
/// root notes → `_resources/f`, 1 → `../_resources/f`, …). Ids without a
/// mapped filename (missing local blob) are left untouched so the export
/// never silently drops a reference.
#[must_use]
pub fn rewrite_ids_to_relative(
    content: &str,
    filename_by_id: &HashMap<String, String>,
    depth: usize,
) -> String {
    if filename_by_id.is_empty() || !content.contains("](:/") {
        return content.to_string();
    }
    let prefix: String = "../".repeat(depth) + "_resources/";
    let mut out: Vec<u8> = Vec::with_capacity(content.len());
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 5 < bytes.len()
            && bytes[i] == b']'
            && bytes[i + 1] == b'('
            && bytes[i + 2] == b':'
            && bytes[i + 3] == b'/'
        {
            let start = i + 4;
            let mut end = start;
            while end < bytes.len()
                && !matches!(bytes[end], b')' | b'"' | b' ' | b'\t' | b'\n' | b'\r')
            {
                end += 1;
            }
            let id = std::str::from_utf8(&bytes[start..end]).unwrap_or("");
            if let Some(filename) = filename_by_id.get(id) {
                out.extend_from_slice(b"](");
                out.extend_from_slice(prefix.as_bytes());
                out.extend_from_slice(filename.as_bytes());
                i = end;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| content.to_string())
}

/// One rewritten import link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRewrite {
    /// Note body with `_resources/` destinations replaced by `:/<id>`.
    pub content: String,
    /// Basenames that looked like resource links but had no file.
    pub missing: usize,
}

/// Rewrite export-relative `_resources/` destinations to internal `:/<id>`
/// using the import's basename→id map. Web/anchor/data links are ignored;
/// `.md`/`.html` links are linked notes, not resources, and pass through.
#[must_use]
pub fn rewrite_relative_to_ids(
    content: &str,
    id_by_basename: &HashMap<String, String>,
) -> ImportRewrite {
    // No `_resources/` destinations at all: nothing to do. Note the map may
    // legitimately be empty (every blob skipped) while links still dangle —
    // those count as missing below.
    if !content.contains("_resources") {
        return ImportRewrite {
            content: content.to_string(),
            missing: 0,
        };
    }
    let mut out: Vec<u8> = Vec::with_capacity(content.len());
    let mut missing = 0;
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b']' && i + 1 < bytes.len() && bytes[i + 1] == b'(' {
            let dest_start = i + 2;
            let mut dest_end = dest_start;
            while dest_end < bytes.len()
                && !matches!(bytes[dest_end], b')' | b'"' | b' ' | b'\t' | b'\n' | b'\r')
            {
                dest_end += 1;
            }
            let dest = std::str::from_utf8(&bytes[dest_start..dest_end]).unwrap_or("");
            let lower = dest.to_ascii_lowercase();
            let ignorable = dest.is_empty()
                || lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("data:")
                || lower.starts_with('#')
                || lower.starts_with("mailto:");
            if !ignorable && dest.contains("_resources") {
                let basename = dest.rsplit(['/', '\\']).next().unwrap_or(dest);
                let base_lower = basename.to_ascii_lowercase();
                if base_lower.ends_with(".md") || base_lower.ends_with(".html") {
                    // Linked note — not a resource.
                } else if let Some(id) = id_by_basename.get(basename) {
                    out.extend_from_slice(b"](:/");
                    out.extend_from_slice(id.as_bytes());
                    i = dest_end;
                    continue;
                } else {
                    missing += 1;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    ImportRewrite {
        content: String::from_utf8(out).unwrap_or_else(|_| content.to_string()),
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ids_are_32_hex_chars() {
        assert!(is_valid_resource_id("b1a3993f0b0359c1603f6d0115809546"));
        assert!(!is_valid_resource_id(""));
        assert!(!is_valid_resource_id("short"));
        assert!(!is_valid_resource_id(
            "b1a3993f0b0359c1603f6d0115809546-extra"
        ));
        assert!(!is_valid_resource_id("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"));
    }

    #[test]
    fn id_prefix_splits_lossless_names() {
        let (id, orig) = split_id_prefix("b1a3993f0b0359c1603f6d0115809546-picture.png").unwrap();
        assert_eq!(id, "b1a3993f0b0359c1603f6d0115809546");
        assert_eq!(orig, "picture.png");
        assert!(split_id_prefix("picture.png").is_none());
        assert!(split_id_prefix("short-name.png").is_none());
    }

    #[test]
    fn export_filename_keeps_id_prefix() {
        assert_eq!(
            export_filename("abcdef0123456789abcdef0123456789", "a/b.png"),
            "abcdef0123456789abcdef0123456789-a-b.png"
        );
    }

    #[test]
    fn extracts_internal_links_in_order() {
        let body = "![a](:/idone12345678901234567890123456) and [b](:/idtwo12345678901234567890123456 \"t\")";
        let ids = extract_resource_ids(body);
        assert_eq!(
            ids,
            vec![
                "idone12345678901234567890123456".to_string(),
                "idtwo12345678901234567890123456".to_string()
            ]
        );
        assert!(extract_resource_ids("no links [x](https://e.com) here").is_empty());
    }

    #[test]
    fn export_rewrite_uses_depth_prefix() {
        let mut map = HashMap::new();
        map.insert("abc".to_string(), "abc-pic.png".to_string());
        let body = "![p](:/abc)";
        assert_eq!(
            rewrite_ids_to_relative(body, &map, 0),
            "![p](_resources/abc-pic.png)"
        );
        assert_eq!(
            rewrite_ids_to_relative(body, &map, 2),
            "![p](../../_resources/abc-pic.png)"
        );
        // Unknown ids stay untouched.
        assert_eq!(
            rewrite_ids_to_relative("![p](:/nope)", &map, 0),
            "![p](:/nope)"
        );
    }

    #[test]
    fn import_rewrite_maps_basename_and_counts_missing() {
        let mut map = HashMap::new();
        map.insert("abc-pic.png".to_string(), "abc".to_string());
        let r = rewrite_relative_to_ids("![p](../_resources/abc-pic.png)", &map);
        assert_eq!(r.content, "![p](:/abc)");
        assert_eq!(r.missing, 0);
        let r = rewrite_relative_to_ids("![p](../_resources/gone.png)", &map);
        assert_eq!(r.missing, 1);
        // Linked notes pass through.
        let r = rewrite_relative_to_ids("[n](../_resources/other.md)", &map);
        assert_eq!(r.missing, 0);
    }
}
