# Notas

A fast, local-first note-taking app for Linux, written in Rust.

## Tech stack

- [Relm4](https://relm4.org/) (0.11) — Elm-style GTK4 bindings
- GTK4 + libadwaita (adaptive three-pane layout)
- GtkSourceView5 — Markdown editing with syntax highlighting
- SQLite via [sqlx](https://github.com/launchbadge/sqlx) (tokio runtime) — single
  database file at `$XDG_DATA_HOME/notas/notas.db`, FTS5 full-text search
- [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) — lightweight
  Markdown preview (no webview)

## Architecture

Workspace with two crates:

- `notas-core` — pure data layer: schema, repository, FTS5 search, export,
  backup, and the sync *planner* (pure conflict/deletion logic, fully
  unit-tested). No GTK. Fully unit-tested (`cargo test -p notas-core`).
- `notas` — the GTK app. The UI thread never touches SQLite directly: every
  database operation goes through an async worker (`DbWorker`) that runs on
  relm4's tokio runtime, keeping the UI responsive. The S3 I/O executor
  (`sync.rs`) also runs on that worker, so a manual sync never blocks the
  UI either.

## Building

System requirements: GTK4, libadwaita, GtkSourceView 5 development files.

```bash
cargo build --release
./target/release/notas
```

## Development

```bash
# Run the full test suite (core + Markdown renderer; GUI probes are #[ignore]d)
cargo test --workspace

# Lint the whole workspace with warnings as errors
cargo clippy --workspace --all-targets -- -D warnings
```

## Features (v1)

- Notebooks (nested tree) + tags with filtering
- Markdown editing with syntax highlighting, toggleable rendered preview
- Full-text search (FTS5, search-as-you-type with hit highlighting)
- Manual save (Ctrl+S) with dirty indicator and save/discard prompts on switch
- Find & replace (Ctrl+F, F3, Replace all)
- Trash with restore / permanent delete
- Markdown export (safety valve for the DB-only storage)
- One-click SQLite backup (`VACUUM INTO`)
- Manual sync to any S3-compatible store (AWS S3, Cloudflare R2, Backblaze
  B2, MinIO, …) — ☰ menu → “Sync now…”; configuration under Settings → Sync
- Settings (☰ menu → Settings…): theme (follow system / light / dark,
  applied immediately), line-number gutter, status bar, sync target &
  credentials — persisted to `$XDG_CONFIG_HOME/notas/settings.json`
- Keyboard shortcuts: Ctrl+N new note, Ctrl+S save, Ctrl+F find, Ctrl+Shift+F
  search notes, Ctrl+E preview, F3 next match, Ctrl+Q quit

## i18n

All user-visible strings are wrapped in `tr!()` (see `notas/src/tr.rs`), so
switching to gettext later is a mechanical change.

## Sync

Sync is **manual**: ☰ menu → “Sync now…” pushes local changes up and pulls
remote changes down in one pass. There is no background auto-sync yet.
The status bar shows the sync state at a glance: “Syncing…” while a sync
runs, and the time of the last successful sync afterwards.

Each note is identified across devices by a stable uuid (assigned on first
sync; the SQLite `notes.id` never leaves the machine). On the bucket, under
the configured key prefix (default `notas/`), every note is a pair of
objects:

```
notes/<uuid>.md        — the Markdown body
meta/<uuid>.json       — sidecar: title, notebook path, tags, trash state,
                         updated_at, content hash
```

### Conflict and deletion rules

- **Last-write-wins** on `updated_at` (UTC, second resolution).
- Same-second *content* collision: both texts are kept — the local note
  stays, and the remote version is imported as a new “`<title> (conflict
  copy)`” note.
- Same-second metadata-only difference (title, notebook, tags, trash): the
deterministically smaller tuple wins on every device, so state converges
instead of oscillating.
- **Trash propagates** both ways (it's just a state in the sidecar).
- **“Delete forever” does not propagate as a deletion**: it uploads a
tombstone sidecar, and other devices move their copy to the trash instead
of deleting it — a permanent delete on one device never destroys the note
elsewhere. Deleting a note on *all* devices requires deleting each copy.
  Restoring a note on one device resurrects it everywhere.

### Known limitations (v1)

- Notebook and tag **renames** only propagate once a contained note is
  edited (the sidecar carries the *current* names at upload time).
- If you're editing a note while syncing, the save happens first; the open
  editor buffer is not re-synced in place (switch notes to reload).
- Two notes with the same title but different uuids are two separate notes
  and are never merged.
- S3 listings are paginated, so thousands of notes are fine.

### Security

The access key and secret are stored **in plaintext** in
`settings.json` (like Joplin does), entered in the UI. Use a dedicated
credential restricted to the sync bucket only, so a leaked settings file
only exposes this one bucket and can be revoked independently.

## Roadmap (v2)

Backlinks (`[[wikilink]]`), attachments/images, spell checking, command
palette, multiple editor tabs, custom incremental syntax highlighting,
automatic / real-time sync.
