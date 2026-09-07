# Notas

**A fast, local-first note-taking app for Linux, written in Rust.**

Notas keeps all your notes in a single SQLite database on your machine — no account, no cloud required. It pairs a smooth GTK4 Markdown editor with full-text search and optional end-to-end encrypted sync to any S3-compatible or WebDAV backend.

## Features

- **Notebooks & tags** — nested notebook tree with tag filtering
- **Markdown editing** — syntax highlighting plus a toggleable rendered preview that renders in-process (no webview)
- **Full-text search** — SQLite FTS5, search-as-you-type with hit highlighting
- **Find & replace** — Ctrl+F, F3 for next match, Replace all
- **Manual save** — Ctrl+S with a dirty indicator and save/discard prompts when switching notes
- **Trash** — restore or permanently delete
- **Export & backup** — Markdown export and one-click SQLite backup (`VACUUM INTO`)
- **Sync (manual, optional)** — push/pull to any S3-compatible store or WebDAV server, with optional end-to-end encryption
- **Settings** — theme (follow system / light / dark, applied immediately), line-number gutter, status bar, sync backend & credentials
- **Keyboard shortcuts** — Ctrl+N new note, Ctrl+S save, Ctrl+F find, Ctrl+Shift+F search notes, Ctrl+E preview, F3 next match, Ctrl+Q quit

## Why local-first

- Your notes live in **one SQLite file** at `$XDG_DATA_HOME/notas/notas.db` — fully searchable (FTS5), no account needed.
- The UI thread **never touches the database directly**: every operation goes through an async `DbWorker` on relm4's tokio runtime, so the app stays responsive even while syncing.
- The Markdown preview renders in-process with [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) — no webview, no JavaScript.

## Building

System requirements: GTK4, libadwaita, and GtkSourceView 5 development files.

```bash
cargo build --release
./target/release/notas
```

## Architecture

Notas is a single Cargo package with **two targets that share one `src/` tree**:

- **`notas` (library, `src/lib.rs`)** — the platform-neutral API: `core`, `application`, `markdown` and `sync`. It compiles with no GTK toolchain and is what the integration tests in `tests/` link against.
- **`notas` (binary, `src/main.rs`)** — the GTK frontend (`app`, `editor`, `notes`, `ui`, `platform`, `tr`). It is a thin shell: it opens the database, loads settings, runs `RelmApp`, and re-exports the library modules (`pub use notas::{application, core, markdown, sync}`) so the whole process uses a single copy of the core types instead of compiling them twice.

The data core is deliberately GUI- and platform-free. All GTK/Relm4 dependencies sit behind the `gui` feature (enabled by default, required to build the binary), so the library builds and tests without them.

### Module map

Each line below states one file's boundary — what it owns, and (where it matters) what it must not touch. Dependency arrows point _down_; a module may only use modules listed below it or to its right in the map.

```text
src/
├── lib.rs            # Library root. #![deny(missing_docs)]. Exports application, core,
│                     #   markdown, sync only — never the GUI modules.
├── main.rs           # Binary root (gui feature). Opens tokio runtime + SQLite pool, loads
│                     #   Settings, runs RelmApp; declares the frontend modules below.
│
├── core/             # ── GUI/platform-free data core ─────────────────────────────────
│   │                 # Depends only on std, serde, sqlx, tokio. No GTK, no network,
│   │                 #   no env lookup, no UI strings, no logging.
│   ├── mod.rs        # Narrow facade; module docs; declares the submodules.
│   ├── error.rs      # Typed Error enum + Result alias for the whole core: database,
│   │                 #   I/O, serialization, migration, NotFound, invalid input.
│   ├── model.rs      # Row types mirroring the schema (Note/Notebook/Tag/TagCount/
│   │                 #   SearchHit) + typed ids (NoteId/NotebookId/TagId) so ids cannot
│   │                 #   be mixed up at call sites.
│   ├── database/     # SQLite lifecycle only.
│   │   ├── mod.rs    # connect(): open/create pool (WAL, busy timeout), then run schema,
│   │   │             #   migrations, and rebuild FTS when it desynchronizes.
│   │   ├── schema.rs # Canonical idempotent DDL (SCHEMA_SQL): tables, unique indexes,
│   │   │             #   FTS5 virtual table + keep-in-sync triggers, version table.
│   │   └── migrations.rs # Ordered, transactional, idempotent upgrades (v0 -> v1 adds
│   │                     #   notes.uuid) + version bookkeeping + post-migration indexes.
│   ├── repository/   # Every SQLite read/write behind one concrete Repository struct
│   │                 #   (no trait: exactly one storage engine exists today).
│   │   ├── mod.rs    # Repository facade over SqlitePool; declares the shared transaction
│   │   │             #   boundaries (remote-note apply, tag replacement, delete+tombstone).
│   │   ├── notes.rs  # Note CRUD, trash/restore, delete-forever (writes a sync tombstone),
│   │   │             #   list queries; rows_affected -> typed NotFound helper.
│   │   ├── notebooks.rs # Notebook CRUD under the unique (parent,name) constraint; name
│   │   │                #   validation; UNIQUE-violation -> NotebookNameExists mapping.
│   │   ├── tags.rs   # Tag list with counts, per-note tags, set-replace (creates tags on
│   │   │             #   demand; shared by the UI and the remote-apply path), rename/delete.
│   │   ├── sync_state.rs # Sync persistence: uuid assignment, local sync index (notebook
│   │   │             #   paths + tags), tombstone list, notebook-path resolve-or-create,
│   │   │             #   apply_remote_note, conflict copy, no-bump trash, last-sync time.
│   │   ├── search.rs # The FTS5 SELECT itself; builds its MATCH string via core::search.
│   │   ├── export.rs # Pure planning (plan/sanitize_component/yaml_scalar/frontmatter,
│   │   │             #   per-directory collision counts) + the filesystem writer run().
│   │   └── backup.rs # VACUUM INTO snapshot; path embedded (cannot bind) after escaping.
│   ├── search.rs     # Pure FTS5 MATCH escaping (fts_query): user input -> safe phrase
│   │                 #   query. Exhaustively unit-tested; no I/O.
│   └── sync/         # Pure conflict/deletion domain; no database, network or time calls.
│       ├── mod.rs    # Re-exports the planner vocabulary + content_hash.
│       ├── model.rs  # Value types: SyncUuid, Sidecar (+ validated() invariants), LocalNote,
│       │             #   RemoteEntry, SyncAction, SyncStats, normalize_tags.
│       └── planner.rs # plan_sync(): deterministic last-write-wins / conflict-copy /
│                      #   tombstone decisions + content_hash. The executor executes them.
│
├── application/      # ── GUI-neutral application protocol (library) ────────────────
│   │                 # Depends on core only. No GTK, no SQL, no remote-protocol details.
│   ├── mod.rs        # Declares commands/events/config/navigation/state; compat aliases
│   │                 #   DbMsg/AppMsg used by the GTK worker/widgets.
│   ├── commands.rs   # DbCommand (worker intents, incl. SyncNow) + AppCommand (semantic
│   │                 #   frontend messages the widgets emit).
│   ├── events.rs     # DbEvent results sent back to a frontend; error payloads are
│   │                 #   already-rendered display strings (the UI seam's decision).
│   ├── config.rs     # Settings model + atomic load/save; theme/editor/interface/sync
│   │                 #   sections; defaults on corrupt or missing files.
│   ├── navigation.rs # ViewId / ViewMode.
│   └── state.rs      # Pure state transitions: editor dirty check, view -> mode mapping.
│
├── markdown.rs       # Frontend-neutral Markdown projection (library): pulldown-cmark
│                     #   events -> styled Span stream (Style/Span/RenderedMarkdown/Stateful
│                     #   Renderer). No GTK; the editor preview and tests both consume it.
│
├── sync/             # ── Sync infrastructure adapters (library) ─────────────────────
│   │                 # Depends on core (repository + core::sync) and application::config
│   │                 #   only; core never depends back on it.
│   ├── mod.rs        # Declares crypto/error/executor; re-exports core::sync vocabulary.
│   ├── error.rs      # SyncError: typed transport/database/repository/crypto/config errors.
│   ├── crypto.rs     # Encryption primitives: Cipher/Verifier, XChaCha20-Poly1305 blobs,
│   │                 #   Argon2id key derivation, wrong-password check.
│   └── executor.rs   # S3 + WebDAV backends behind the private SyncStore trait (Send+Sync);
│                     #   shared run loop: resolve cipher -> list both indexes -> plan_sync
│                     #   -> execute actions -> record timestamp. Wiremock WebDAV tests.
│
└── GUI (binary only, behind the `gui` feature; consumes core/application results, never
    touches SQLite or the network directly):
    ├── app/          # GTK coordinator + async worker.
    │   ├── mod.rs    # App SimpleComponent: owns widgets + app state (current note, dirty
    │   │             #   flag, view mode, caches), builds the three-pane window, wires
    │   │             #   keyboard shortcuts and the close dialog; routes to messages.rs.
    │   ├── actions.rs # Note-lifecycle actions + widget-rebuild helpers (impl App)
    │   │             #   called by the message handlers below.
    │   ├── messages.rs # App::handle (every AppMsg) and handle_db_event (every DbEvent):
    │   │             #   pure translation into model updates + worker requests.
    │   ├── layout.rs # Initial 1:1:2 divider split (GtkPaned) + its display probes.
    │   └── db_worker.rs # DbWorker (relm4 Worker): the ONLY component allowed to run
    │                   #   Repository/sync calls from the GUI side; converts typed core
    │                   #   errors into user-facing text at this boundary.
    ├── editor/       # GtkSourceView editor pane.
    │   ├── mod.rs    # Editor struct (source buffer/view, preview buffer, find/replace
    │   │             #   bar) + build_editor().
    │   └── preview.rs # Text-tag palette (PreviewTags) + renders markdown::Span into the
    │                 #   preview TextBuffer; theme application; visual probes.
    ├── notes/        # GTK note list / sidebar / notebook-tree / tag widgets.
    │   ├── mod.rs    # Shared row builder + list/flow clear helpers.
    │   ├── list.rs   # Renders Note and SearchHit rows into the middle note list.
    │   ├── sidebar.rs # Search entry + built-in views + tag chips + context menu;
    │   │             #   composes the notebook pane into the sidebar.
    │   ├── notebook.rs # Notebook pane: TreeView (deprecated GTK API, tracked migration),
    │   │             #   new/rename/delete menu, tree rebuild from core::model::Notebook.
    │   └── tag_editor.rs # Removable tag chips under the editor title bar.
    ├── ui/           # libadwaita dialogs, settings, presentation helpers.
    │   ├── mod.rs    # Re-exports the page/dialog modules below.
    │   ├── protocol.rs # GTK-free re-export point for AppMsg/ViewMode (frontend protocol).
    │   ├── dialogs.rs # AlertDialog/GtkDialog helpers: unsaved-changes, confirm, error,
    │   │             #   text input.
    │   ├── status.rs # Status-bar text helpers (e.g. the sync indicator).
    │   ├── theme.rs  # Applies ThemeMode to libadwaita's StyleManager.
    │   └── settings/ # Settings PreferencesDialog (immediate-effect rows -> AppMsg).
    │       ├── mod.rs        # Assembles the dialog from the two pages.
    │       ├── appearance.rs # Theme + editor toggles (line numbers, status bar).
    │       └── sync.rs       # Backend selector, per-backend credential rows,
    │                         #   encryption switch with password confirmation.
    ├── platform/     # OS adapters (frontend crate, GTK-free): XDG/HOME/APPDATA -> paths.
    │   ├── mod.rs    # PlatformPaths + cfg-dispatched paths() used by main.rs only.
    │   ├── linux.rs  # XDG_CONFIG_HOME / XDG_DATA_HOME (or HOME fallbacks).
    │   ├── macos.rs  # ~/Library/Application Support/Notas.
    │   └── windows.rs # %APPDATA% / %LOCALAPPDATA%.
    └── tr.rs         # tr!() macro (returns the literal today; the gettext swap point).
                      #   UI strings only — never used from core/application/markdown/sync.
```

### Dependency and boundary rules

- **Direction.** `frontend (app/editor/notes/ui) -> app coordinator -> application -> core`; `sync` is infrastructure beside `application`, depending on `core` + `application::config`; `platform` is consumed only by `main.rs`; `markdown` is a leaf consumed by the preview and tests. Core never imports `application`, `sync`, GTK, or platform code — this is what makes the library buildable headless.
- **One database owner per side.** Every SQL statement lives in `core::repository`; the GUI side reaches it only through `DbWorker`, and sync through the `Repository` reference the worker passes to `run_sync`. Widgets never touch the pool.
- **Network only in `sync/executor`; crypto only in `sync/crypto`.** `core::sync::planner` performs no I/O at all — it is a pure decision function, which is why its conflict/tombstone rules are unit-testable without a backend.
- **Errors stay typed until the UI seam.** `core::error::Error`, `sync::error::SyncError` and `core::sync::model::SidecarError` are `thiserror` enums; they are converted to display strings only in `db_worker.rs` / `dialogs.rs`. `anyhow` is deliberately not used (it would erase the typed context this layered design needs).
- **UI strings only from the GUI.** `tr!` is imported only by `app/`, `editor/`, `notes/` and `ui/`; the library layers keep user-facing wording out of their messages (error `Display`s are the single exception, rendered at the seam).
- **No premature abstraction.** `Repository` is a concrete struct (one storage engine exists); sync backends implement the private `SyncStore` trait because two engines genuinely share a run loop.
- **Lints are enforced.** `#![deny(missing_docs)]` on the library crate and `[lints.clippy] all = deny` in `Cargo.toml`, so the CI `clippy -D warnings` gate and the doc comments above are both part of the contract.

### Testing across the boundary

```bash
cargo test --lib --no-default-features   # core + application + markdown + sync, no GTK
cargo test --all-targets --all-features  # everything incl. the GUI binary's headless tests
```

Unit tests for pure logic (`fts_query`, export `plan`, sync `plan_sync`, markdown `render`, settings serde, …) live in `#[cfg(test)]` blocks next to the code. Integration tests under `tests/` go through the public library API: `core_data.rs` (schema upgrades / start-ups), `core_search.rs` (FTS end-to-end), `core_export_backup.rs` (filesystem export + `VACUUM INTO` backup), `repository.rs` (CRUD round-trips). The GUI binary's display probes (`settings_window_probe`, pane scroll/editable checks, …) are `#[ignore]`d and need a real display.

## Sync

Sync is **manual**: ☰ menu → “Sync now…” pushes local changes up and pulls remote changes down in one pass. There is no background auto-sync. The status bar shows the state at a glance — “Syncing…” while a sync runs, and the time of the last successful sync afterwards.

Each note is identified across devices by a stable UUID (assigned on first sync; the SQLite `notes.id` never leaves the machine). Under the configured prefix / directory, every note is a pair of objects — the layout is identical for both backends:

```
notes/<uuid>.md        — the Markdown body
meta/<uuid>.json       — sidecar: title, notebook path, tags, trash state, updated_at, content hash
meta/.encryption-verifier — plaintext metadata about the encryption (salt + password check), only present when encryption is enabled
```

### Backends

**S3-compatible object storage** — AWS S3, Cloudflare R2, Backblaze B2, MinIO, ... Configure the endpoint (empty = AWS), region, bucket, key prefix (default `notas/`), and an access key/secret. The bucket itself must exist already.

**WebDAV** — Nextcloud, ownCloud, and any RFC-4918 server. Configure:

- _Server URL_ — a DAV collection you can write to. For Nextcloud that is `https://<host>/remote.php/dav/files/<user>`.
- _Directory_ (default `notas`) — the folder under that URL where the notes live; it is created automatically on the first sync. Empty the field to sync straight into the URL itself.
- _Username_ and _password_ (Basic auth). For Nextcloud, create an **app password** in your account settings and use that — it can be revoked without touching your main password.
- _Allow insecure TLS_ (off by default) — accepts self-signed / invalid certificates and plain `http://` URLs. Only enable this for a server you trust on a network you trust: with it on, credentials can be read in transit and the server's identity is not verified.

The settings dialog shows the fields of the selected backend; both backends' configuration is kept, so switching never loses what you entered.

### Encryption

With **Settings → Sync → Encryption → “Encrypt synced notes”** on, every uploaded object (bodies, sidecars, tombstones) is sealed with **XChaCha20-Poly1305** under a key derived from your password via **Argon2id**. The backend only ever stores ciphertext plus random UUIDs.

- Enter the **same password on every device** that syncs: each device derives the same key from the same password and salt, so notes encrypted on one device decrypt on another. There is **no password recovery** — a lost password means the backend data is unreadable.
- Enabling encryption **re-uploads everything**: the first sync after enabling writes every local note encrypted over its plaintext copy on the backend (and writes the verifier object). Enable it on a device that holds the notes you want to keep.
- A wrong password is caught **before any note traffic**: the sync reads the verifier object first, derives the key, and aborts with a clear error on mismatch, instead of overwriting encrypted data with garbage.
- Disabling encryption asks for the current password, then re-uploads everything in plaintext on the next sync. A device that syncs with encryption off against an encrypted backend is refused.
- The password is stored **in plaintext in the settings file**, like the sync credentials (Joplin-style): it protects against _backend_ compromise, not against theft of this machine.
- The Argon2id salt is not secret; it travels in the verifier object and in every blob header, so deleting the verifier object does not lose data (the salt is recovered from the objects themselves).
- S3 buckets with **object versioning** enabled may retain the old plaintext versions after re-encryption — purge old versions if the backend must not see plaintext at all.

### Conflict and deletion rules

- **Last-write-wins** on `updated_at` (UTC, second resolution).
- Same-second _content_ collision: both texts are kept — the local note stays, and the remote version is imported as a new “`<title> (conflict copy)`” note.
- Same-second metadata-only difference (title, notebook, tags, trash): the deterministically smaller tuple wins on every device, so state converges instead of oscillating.
- **Trash propagates** both ways (it's just a state in the sidecar).
- **“Delete forever” does not propagate as a deletion**: it uploads a tombstone sidecar, and other devices move their copy to the trash instead of deleting it — a permanent delete on one device never destroys the note elsewhere. Restoring a note on one device resurrects it everywhere; deleting a note on _all_ devices requires deleting each copy.

### Security

The access key / password are stored **in plaintext** in `settings.json` (like Joplin), entered in the UI. Use scoped credentials: a dedicated S3 key restricted to the sync bucket only, or a Nextcloud app password — so a leaked settings file only exposes that one bucket/account and can be revoked independently.

## Known limitations

- Notebook and tag **renames** only propagate once a contained note is edited (the sidecar carries the _current_ names at upload time).
- If you're editing a note while syncing, the save happens first; the open editor buffer is not re-synced in place (switch notes to reload).
- Two notes with the same title but different UUIDs are two separate notes and are never merged.
- S3 listings are paginated and WebDAV folders are listed depth-1, so thousands of notes are fine either way.
- WebDAV auth is Basic only (Digest is not implemented yet).

## Development

```bash
# Run the full test suite (core + Markdown renderer; GUI probes are #[ignore]d)
cargo test --all-targets --all-features --locked

# Lint the package with warnings as errors
cargo clippy --all-targets --all-features --locked -- -D warnings
```

## Tech stack

- [Relm4](https://relm4.org/) (0.11) — Elm-style GTK4 bindings
- GTK4 + libadwaita (adaptive three-pane layout)
- GtkSourceView5 — Markdown editing with syntax highlighting
- SQLite via [sqlx](https://github.com/launchbadge/sqlx) (tokio runtime) — single database file at `$XDG_DATA_HOME/notas/notas.db`, FTS5 full-text search
- [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) — lightweight Markdown preview (no webview)

## i18n

All user-visible strings are wrapped in `tr!()` (see `notas/src/tr.rs`), so switching to gettext later is a mechanical change.

## License

Notas is released under the [MIT License](LICENSE).

## Acknowledgments

- **[Joplin](https://joplinapp.org/)** — the inspiration for Notas. Its local-first philosophy, plaintext-credential settings file, app-password workflow, and end-to-end encryption for synced notes directly shaped Notas' sync design.
- **The tech stack** — Notas stands on the shoulders of these open-source projects:
  - [Relm4](https://relm4.org/), [GTK4](https://www.gtk.org/), [libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/), [GtkSourceView5](https://gitlab.gnome.org/GNOME/gtksourceview) — the UI
  - [sqlx](https://github.com/launchbadge/sqlx) + SQLite — storage and FTS5 search
  - [tokio](https://tokio.rs/) — the async runtime
  - [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) — the Markdown preview
  - [argon2](https://crates.io/crates/argon2) + [chacha20poly1305](https://crates.io/crates/chacha20poly1305) — end-to-end encryption
  - [s3](https://crates.io/crates/s3) + [reqwest_dav](https://crates.io/crates/reqwest_dav) — the sync backends
  - [serde](https://serde.rs/) — serialization
