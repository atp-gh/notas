# Development

```bash
# Run the full test suite (core + Markdown renderer; GUI probes are #[ignore]d)
cargo test --all-targets --all-features --locked

# Lint the package with warnings as errors
cargo clippy --all-targets --all-features --locked -- -D warnings
```

## Testing across the boundary

```bash
cargo test --lib --no-default-features   # core + application + markdown + sync, no GTK
cargo test --all-targets --all-features  # everything incl. the GUI binary's headless tests
```

Unit tests for pure logic (`fts_query`, export `plan`, sync `plan_sync`, markdown `render`, settings serde, …) live in `#[cfg(test)]` blocks next to the code. Integration tests under `tests/` go through the public library API: `core_data.rs` (schema upgrades / start-ups), `core_search.rs` (FTS end-to-end), `core_export_backup.rs` (filesystem export + `VACUUM INTO` backup), `repository.rs` (CRUD round-trips). The GUI binary's display probes (`settings_window_probe`, pane scroll/editable checks, …) are `#[ignore]`d and need a real display.

## Tech stack

- [Relm4](https://relm4.org/) (0.11) — Elm-style GTK4 bindings
- GTK4 + libadwaita (adaptive three-pane layout)
- GtkSourceView5 — Markdown editing with syntax highlighting
- SQLite via [sqlx](https://github.com/launchbadge/sqlx) (tokio runtime) — single database file at `$XDG_DATA_HOME/notas/notas.db`, FTS5 full-text search
- [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) — lightweight Markdown preview (no webview)

## i18n

All user-visible strings are wrapped in `tr!()` (see `notas/src/tr.rs`), so switching to gettext later is a mechanical change.
