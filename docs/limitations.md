# Known limitations

- Attachments are capped at **100 MiB** per file (add/import refuses larger
  ones); sync uploads/downloads whole blobs with no resume, so very large
  attachments need a patient 30 s-timeout network.
- A re-import of an export without front matter (no Joplin `id`) duplicates notes instead of updating them; front-matter exports are idempotent by design.
- Notebook and tag **renames** only propagate once a contained note is edited (the sidecar carries the _current_ names at upload time).
- If you're editing a note while syncing, the save happens first; the open editor buffer is not re-synced in place (switch notes to reload).
- Two notes with the same title but different UUIDs are two separate notes and are never merged.
- S3 listings are paginated and WebDAV folders are listed depth-1, so thousands of notes are fine either way.
- WebDAV auth is Basic only (Digest is not implemented yet).
