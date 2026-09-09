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
- **Import from Joplin** — import a full “Export all as Markdown” directory (with or without front matter): the notebook tree is recreated (existing same-name notebooks are merged), timestamps and tags are preserved, and re-importing updates notes by their Joplin id
- **Export & backup** — Markdown export and one-click SQLite backup (`VACUUM INTO`)
- **Sync (manual, optional)** — push/pull to any S3-compatible store or WebDAV server, with optional end-to-end encryption
- **Settings** — theme (follow system / light / dark, applied immediately), line-number gutter, status bar, sync backend & credentials
- **Keyboard shortcuts** — Ctrl+N new note, Ctrl+S save, Ctrl+F find, Ctrl+Shift+F search notes, Ctrl+E preview, F3 next match, Ctrl+Q quit

## Building

System requirements: GTK4, libadwaita, and GtkSourceView 5 development files.

```bash
cargo build --release
./target/release/notas
```

## Sync

Sync is **manual** (☰ menu → “Sync now…”) to any S3-compatible store or WebDAV server, with optional end-to-end encryption (XChaCha20-Poly1305 + Argon2id). See [docs/sync.md](docs/sync.md) for backends, encryption, and conflict rules.

## Docs

- [docs/architecture.md](docs/architecture.md) — module map and boundary rules
- [docs/sync.md](docs/sync.md) — sync backends, encryption, conflict rules
- [docs/development.md](docs/development.md) — tests, lint, tech stack
- [docs/limitations.md](docs/limitations.md) — known limitations

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
