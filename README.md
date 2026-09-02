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
  backup. No GTK. Fully unit-tested (`cargo test -p notas-core`).
- `notas` — the GTK app. The UI thread never touches SQLite directly: every
  database operation goes through an async worker (`DbWorker`) that runs on
  relm4's tokio runtime, keeping the UI responsive.

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
- Settings (☰ menu → Settings…): theme (follow system / light / dark,
  applied immediately), line-number gutter, status bar — persisted to
  `$XDG_CONFIG_HOME/notas/settings.json`
- Keyboard shortcuts: Ctrl+N new note, Ctrl+S save, Ctrl+F find, Ctrl+Shift+F
  search notes, Ctrl+E preview, F3 next match, Ctrl+Q quit

## i18n

All user-visible strings are wrapped in `tr!()` (see `notas/src/tr.rs`), so
switching to gettext later is a mechanical change.

## Roadmap (v2)

Backlinks (`[[wikilink]]`), attachments/images, spell checking, command
palette, multiple editor tabs, custom incremental syntax highlighting,
sync.
