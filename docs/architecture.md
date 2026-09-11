# Architecture

## Why local-first

- Your notes live in **one SQLite file** at `$XDG_DATA_HOME/notas/notas.db` — fully searchable (FTS5), no account needed.
- The UI thread **never touches the database directly**: every operation goes through an async `DbWorker` on relm4's tokio runtime, so the app stays responsive even while syncing.
- The Markdown preview renders in-process with [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) — no webview, no JavaScript.

## Layout

Notas is a single Cargo package with **two targets that share one `src/` tree**:

- **`notas` (library, `src/lib.rs`)** — the platform-neutral API: `core`, `application`, `markdown` and `sync`. It compiles with no GTK toolchain and is what the integration tests in `tests/` link against.
- **`notas` (binary, `src/main.rs`)** — the GTK frontend (`app`, `editor`, `notes`, `ui`, `platform`, `tr`). It is a thin shell: it opens the database, loads settings, runs `RelmApp`, and re-exports the library modules (`pub use notas::{application, core, markdown, sync}`) so the whole process uses a single copy of the core types instead of compiling them twice.

The data core is deliberately GUI- and platform-free. All GTK/Relm4 dependencies sit behind the `gui` feature (enabled by default, required to build the binary), so the library builds and tests without them.

Unit tests for pure logic live next to the code; integration tests under `tests/` go through the public library API (see [development](development.md)).

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
│   │                     #   notes.uuid, v1 -> v2 notebook trash, v2 -> v3 resources)
│   │                     #   + version bookkeeping + post-migration indexes.
│   ├── resources.rs  # Pure attachment domain (no I/O/SQL): id validation, 100 MiB cap,
│   │                 #   `:/<id>` scan/rewrite both directions, `<id>-<name>` export
│   │                 #   mapping, MIME guesses, resources-dir path helpers.
│   ├── repository/   # Every SQLite read/write behind one concrete Repository struct
│   │                 #   (no trait: exactly one storage engine exists today).
│   │   ├── mod.rs    # Repository facade over SqlitePool; declares the shared transaction
│   │   │             #   boundaries (remote-note apply, tag replacement, delete+tombstone).
│   │   ├── notes.rs  # Note CRUD, trash/restore, delete-forever (writes a sync tombstone),
│   │   │             #   list queries; rows_affected -> typed NotFound helper.
│   │   ├── resources.rs # Attachment rows + blob files (`<data_dir>/resources/<uuid>`),
│   │   │             #   reference scan (trashed notes count), idempotent remove.
│   │   ├── notebooks.rs # Notebook CRUD under the unique (parent,name) constraint; name
│   │   │                #   validation; UNIQUE-violation -> NotebookNameExists mapping.
│   │   ├── tags.rs   # Tag list with counts, per-note tags, set-replace (creates tags on
│   │   │             #   demand; shared by the UI and the remote-apply path), rename/delete.
│   │   ├── sync_state.rs # Sync persistence: uuid assignment, local sync index (notebook
│   │   │             #   paths + tags), resource metas + reference set, tombstone list,
│   │   │             #   notebook-path resolve-or-create, apply_remote_note, conflict
│   │   │             #   copy, no-bump trash, last-sync time.
│   │   ├── search.rs # The FTS5 SELECT itself; builds its MATCH string via core::search.
│   │   ├── export.rs # Pure planning (plan/sanitize_component/yaml_scalar/frontmatter,
│   │   │             #   per-directory collision counts) + the filesystem writer run():
│   │   │             #   notes plus a single `_resources/` dir with depth-relative links.
│   │   ├── import.rs # Markdown import (Joplin export layout): hand-rolled front-matter
│   │   │             #   parser, tree walker (incl. `_resources/` blobs with `<id>-`
│   │   │             #   prefix preservation), in-Rust ISO-8601 → SQLite timestamp
│   │   │             #   conversion, transactional upsert runner (notebook merge, uuid
│   │   │             #   update-on-reimport, skip-and-count unreadable files).
│   │   └── backup.rs # VACUUM INTO snapshot; path embedded (cannot bind) after escaping.
│   ├── search.rs     # Pure FTS5 MATCH escaping (fts_query): user input -> safe phrase
│   │                 #   query. Exhaustively unit-tested; no I/O.
│   └── sync/         # Pure conflict/deletion domain; no database, network or time calls.
│       ├── mod.rs    # Re-exports the planner vocabulary + content_hash/bytes_hash.
│       ├── model.rs  # Value types: SyncUuid, Sidecar (+ validated() invariants), LocalNote,
│       │             #   RemoteEntry, SyncAction, SyncStats, ResourceMeta, LocalResource,
│       │             #   RemoteResourceEntry, ResourceAction, normalize_tags.
│       └── planner.rs # plan_sync(): deterministic last-write-wins / conflict-copy /
│                      #   tombstone decisions + content_hash; plan_resources(): referenced
│                      #   upload/download + conservative orphan GC (no tombstones).
│                      #   The executor executes both.
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
│                     #   -> execute actions -> sync_resources (list blobs -> plan_resources
│                     #   -> upload/download/GC, same cipher) -> record timestamp.
│                     #   Wiremock WebDAV tests (incl. encrypted attachments).
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
- **Observability through a facade.** Library layers emit `tracing` events with structured fields (never `eprintln!`); only the binary installs a subscriber (`tracing_subscriber::fmt` with `RUST_LOG` support, default `warn`). `core` stays free of even the facade.
- **No premature abstraction.** `Repository` is a concrete struct (one storage engine exists); sync backends implement the private `SyncStore` trait because two engines genuinely share a run loop.
- **Lints are enforced.** `#![deny(missing_docs)]` on the library crate and `[lints.clippy] all = deny` in `Cargo.toml`, so the CI `clippy -D warnings` gate and the doc comments above are both part of the contract.
