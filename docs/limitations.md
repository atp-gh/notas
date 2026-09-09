# Known limitations

- Imported Joplin **attachments** (`_resources`) are not copied: image and attachment links stay in the note text but do not render, since Notas has no attachment support yet.
- A re-import of an export without front matter (no Joplin `id`) duplicates notes instead of updating them; front-matter exports are idempotent by design.
- Notebook and tag **renames** only propagate once a contained note is edited (the sidecar carries the _current_ names at upload time).
- If you're editing a note while syncing, the save happens first; the open editor buffer is not re-synced in place (switch notes to reload).
- Two notes with the same title but different UUIDs are two separate notes and are never merged.
- S3 listings are paginated and WebDAV folders are listed depth-1, so thousands of notes are fine either way.
- WebDAV auth is Basic only (Digest is not implemented yet).
