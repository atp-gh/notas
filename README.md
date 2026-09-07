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

The application is a single Cargo package with one `src/` tree, layered so that the data core is GUI/platform-free and can be built and tested without GTK:

```text
src/
├── core/            # data core: SQLite database, schema & migrations, repository,
│                    # FTS5 search, pure sync planner. No GTK, no network, no config I/O.
├── application/     # GUI-neutral protocol: commands, events, settings, navigation,
│                    # editor state. Depends on core, embeds no SQL or protocol details.
├── markdown.rs      # frontend-neutral Markdown projection (pulldown-cmark -> styled spans)
├── sync/            # infrastructure adapters: S3/WebDAV executor + crypto. Depends on core.
├── app/             # GTK coordinator: translates commands -> repository/executor calls
├── editor/ notes/ ui/  # GTK widgets (consume application/core results only)
└── platform/        # OS path discovery
```

The dependency direction is `platform/UI -> app coordinator -> application -> core`, with `sync` as an infrastructure adapter into `core`. GTK4, libadwaita, sourceview5, glib, pango and relm4 are optional dependencies behind the `gui` feature (enabled by default), so the data core compiles and tests without a GTK toolchain:

```bash
cargo test --lib --no-default-features   # core + application only, no GTK
cargo test --all-targets --all-features  # everything, including the GUI binary's tests
```

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
