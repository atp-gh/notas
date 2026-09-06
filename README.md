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
  relm4's tokio runtime, keeping the UI responsive. The sync executors
  (`sync.rs`, S3 + WebDAV backends over one shared seam) also run on that
  worker, so a manual sync never blocks the UI either.

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
- One-click SQLite backup (`VACUUM INTO`)- Manual sync to any **S3-compatible store** (AWS S3, Cloudflare R2,
  Backblaze B2, MinIO, …) or any **WebDAV server** (Nextcloud, ownCloud,
  …) — ☰ menu → “Sync now…”;
  configuration under Settings → Sync
- Optional **end-to-end encryption** for synced notes (Settings → Sync →
  Encryption): every object on the backend is sealed with
  XChaCha20-Poly1305 under a key derived from your password via Argon2id
- Settings (☰ menu → Settings…): theme (follow system / light / dark,
  applied immediately), line-number gutter, status bar, sync backend &
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

### Backends

- **S3-compatible object storage** (AWS S3, Cloudflare R2, Backblaze B2,
  MinIO, …). Configure the endpoint (empty = AWS), region, bucket, key
  prefix (default `notas/`), and an access key/secret. The bucket itself
  must exist already.
- **WebDAV** (Nextcloud, ownCloud, and any RFC-4918 server). Configure:
  - *Server URL* — a DAV collection you can write to. For Nextcloud that
    is `https://<host>/remote.php/dav/files/<user>`.
  - *Directory* (default `notas`) — the folder under that URL where the
    notes live; it is created automatically on the first sync. Empty the
    field to sync straight into the URL itself.
  - *Username* and *password* (Basic auth). For Nextcloud, create an
    **app password** in your account settings and use that — it can be
    revoked without touching your main password.
  - *Allow insecure TLS* (off by default) — accepts self-signed/
    invalid certificates and plain `http://` URLs. Only enable this for
    a server you trust on a network you trust: with it on, credentials
    can be read in transit and the server's identity is not verified.

The settings dialog shows the fields of the selected backend; both
backends' configuration is kept, so switching never loses what you
entered.

Each note is identified across devices by a stable uuid (assigned on first
sync; the SQLite `notes.id` never leaves the machine). On the remote
store, under the configured prefix / directory, every note is a pair of
objects — the layout is identical for both backends:

```
notes/<uuid>.md        — the Markdown body
meta/<uuid>.json       — sidecar: title, notebook path, tags, trash state,
                         updated_at, content hash
meta/.encryption-verifier — plaintext metadata about the encryption
                         (salt + password check), only present when
                         encryption is enabled
```

### Encryption

With **Settings → Sync → Encryption → “Encrypt synced notes”** on, every
object uploaded (markdown bodies, sidecars, tombstones) is sealed with
**XChaCha20-Poly1305** under a key derived from the password via
**Argon2id**. The backend only ever stores ciphertext plus random uuids.

- Enter the **same password on every device** that syncs: each device
  derives the same key from the same password and salt, so notes encrypted
  on one device decrypt on another. There is no password recovery — a
  lost password means the backend data is unreadable.
- Enabling encryption **re-uploads everything**: the first sync after
  enabling writes every local note encrypted over its plaintext copy on
  the backend (and writes the verifier object). Enable it on a device
  that holds the notes you want to keep.
- A wrong password is caught **before any note traffic**: the sync reads
  the verifier object first, derives the key, and aborts with a clear
  error on mismatch, instead of overwriting encrypted data with garbage.
- Disabling encryption asks for the current password, then re-uploads
  everything in plaintext on the next sync. A device that syncs with
  encryption off against an encrypted backend is refused.
- The password is stored in **plaintext in the settings file**, like the
  sync credentials (Joplin-style): it protects against *backend* compromise,
  not against theft of this machine.
- The Argon2id salt is not secret; it travels in the verifier object and
  in every blob header, so deleting the verifier object does not lose
  data (the salt is recovered from the objects themselves).
- S3 buckets with **object versioning** enabled may retain the old
  plaintext versions after re-encryption — purge old versions if the
  backend must not see plaintext at all.

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
- S3 listings are paginated and WebDAV folders are listed depth-1, so
  thousands of notes are fine either way.
- WebDAV auth is Basic only (Digest is not implemented yet).

### Security

The access key / password are stored **in plaintext** in
`settings.json` (like Joplin does), entered in the UI. Use scoped
credentials: a dedicated S3 key restricted to the sync bucket only, or a
Nextcloud app password — so a leaked settings file only exposes that one
bucket/account and can be revoked independently.

## Roadmap (v2)

Backlinks (`[[wikilink]]`), attachments/images, spell checking, command
palette, multiple editor tabs, custom incremental syntax highlighting,
automatic / real-time sync.
