//! Markdown preview rendering: the text-tag palette plus the renderer that
//! turns frontend-neutral spans into a `gtk::TextBuffer`.
//!
//! This is the half of the editor pane that owns no widgets of its own
//! beyond the preview buffer's text tags. The `Editor` component and the
//! source/find UI live in `super`, which consumes `PreviewTags` and
//! [`render_markdown`].

use std::cell::RefCell;

use gtk::prelude::*;
use sourceview5::prelude::*;

use crate::markdown::{self, Style};

/// Text tags used by the Markdown preview, created once per buffer.
pub struct PreviewTags {
    pub h1: gtk::TextTag,
    pub h2: gtk::TextTag,
    pub h3: gtk::TextTag,
    pub bold: gtk::TextTag,
    pub italic: gtk::TextTag,
    pub strike: gtk::TextTag,
    pub code: gtk::TextTag,
    pub code_block: gtk::TextTag,
    pub quote_1: gtk::TextTag,
    pub quote_2: gtk::TextTag,
    pub quote_3: gtk::TextTag,
    pub link: gtk::TextTag,
    /// Muted text: link URLs and horizontal rules.
    pub dim: gtk::TextTag,
    /// Monospace, so table columns line up.
    pub table: gtk::TextTag,
    /// Link ranges `(start, end, url)` in the preview buffer, rebuilt on
    /// every render so a click can resolve the URL at a position.
    link_ranges: RefCell<Vec<(i32, i32, String)>>,
}

impl PreviewTags {
    pub fn new(buffer: &gtk::TextBuffer) -> Self {
        // Heading colors/sizes are updated per theme by `apply_theme`;
        // these are the light-theme defaults used before/without that call.
        let h1 = buffer
            .create_tag(
                Some("h1"),
                &[
                    ("size-points", &26f64),
                    ("weight", &800i32),
                    ("foreground", &"#3584e4"),
                    ("pixels-above-lines", &12i32),
                    ("pixels-below-lines", &4i32),
                ],
            )
            .expect("create h1 tag");
        let h2 = buffer
            .create_tag(
                Some("h2"),
                &[
                    ("size-points", &20f64),
                    ("weight", &700i32),
                    ("foreground", &"#3584e4"),
                    ("pixels-above-lines", &10i32),
                    ("pixels-below-lines", &3i32),
                ],
            )
            .expect("create h2 tag");
        let h3 = buffer
            .create_tag(
                Some("h3"),
                &[
                    ("size-points", &16f64),
                    ("weight", &700i32),
                    ("foreground", &"#3584e4"),
                    ("pixels-above-lines", &8i32),
                    ("pixels-below-lines", &2i32),
                ],
            )
            .expect("create h3 tag");
        let bold = buffer
            .create_tag(Some("bold"), &[("weight", &700i32)])
            .expect("create bold tag");
        let italic = buffer
            .create_tag(Some("italic"), &[("style", &pango::Style::Italic)])
            .expect("create italic tag");
        let strike = buffer
            .create_tag(
                Some("strike"),
                &[("strikethrough", &true), ("foreground", &"#7f7f7f")],
            )
            .expect("create strike tag");
        let code = buffer
            .create_tag(
                Some("code"),
                &[
                    ("font", &"monospace"),
                    ("background", &"rgba(127,127,127,0.18)"),
                ],
            )
            .expect("create code tag");
        let code_block = buffer
            .create_tag(
                Some("code-block"),
                &[
                    ("font", &"monospace"),
                    ("background", &"rgba(127,127,127,0.18)"),
                    ("left-margin", &12i32),
                    ("right-margin", &12i32),
                ],
            )
            .expect("create code-block tag");
        let quote_tag = |name: &str, margin: i32| {
            buffer
                .create_tag(
                    Some(name),
                    &[
                        ("foreground", &"#8f8f8f"),
                        ("background", &"rgba(0,0,0,0.05)"),
                        ("left-margin", &margin),
                    ],
                )
                .unwrap_or_else(|| panic!("create {name} tag"))
        };
        let quote_1 = quote_tag("quote-1", 16);
        let quote_2 = quote_tag("quote-2", 32);
        let quote_3 = quote_tag("quote-3", 48);
        let link = buffer
            .create_tag(
                Some("link"),
                &[
                    ("foreground", &"#3584e4"),
                    ("underline", &pango::Underline::Single),
                ],
            )
            .expect("create link tag");
        let dim = buffer
            .create_tag(Some("dim"), &[("foreground", &"#8f8f8f")])
            .expect("create dim tag");
        let table = buffer
            .create_tag(Some("table"), &[("font", &"monospace")])
            .expect("create table tag");

        Self {
            h1,
            h2,
            h3,
            bold,
            italic,
            strike,
            code,
            code_block,
            quote_1,
            quote_2,
            quote_3,
            link,
            dim,
            table,
            link_ranges: RefCell::new(Vec::new()),
        }
    }

    /// Replace the link range table (called on every preview render).
    pub fn set_link_ranges(&self, ranges: Vec<(i32, i32, String)>) {
        *self.link_ranges.borrow_mut() = ranges;
    }

    /// Destination URL of the link covering `offset`, if any.
    pub fn link_url_at(&self, offset: i32) -> Option<String> {
        self.link_ranges
            .borrow()
            .iter()
            .find(|(start, end, _)| offset >= *start && offset < *end)
            .map(|(_, _, url)| url.clone())
    }

    /// Re-apply theme-dependent colors (dark/light palette + accent).
    /// Called once at build time and again whenever the libadwaita theme
    /// or the accent color changes.
    pub fn apply_theme(&self, dark: bool, accent: &gtk::gdk::RGBA) {
        let accent_hex = rgba_to_hex(accent);
        let text_hex = if dark { "#c8c8c8" } else { "#8f8f8f" };
        let quote_bg = if dark {
            "rgba(255,255,255,0.06)"
        } else {
            "rgba(0,0,0,0.05)"
        };
        let code_bg = if dark {
            "rgba(255,255,255,0.10)"
        } else {
            "rgba(127,127,127,0.18)"
        };

        for h in [&self.h1, &self.h2, &self.h3] {
            h.set_property("foreground", accent_hex.as_str());
        }
        self.link.set_property("foreground", accent_hex.as_str());
        for q in [&self.quote_1, &self.quote_2, &self.quote_3] {
            q.set_property("foreground", text_hex);
            q.set_property("background", quote_bg);
        }
        self.dim.set_property("foreground", text_hex);
        self.strike.set_property("foreground", text_hex);
        self.code.set_property("background", code_bg);
        self.code_block.set_property("background", code_bg);
    }
}

/// The text tag that renders a given Markdown style.
fn style_tag(tags: &PreviewTags, style: Style) -> &gtk::TextTag {
    match style {
        Style::Bold => &tags.bold,
        Style::Italic => &tags.italic,
        Style::Strike => &tags.strike,
        Style::Code => &tags.code,
        Style::CodeBlock => &tags.code_block,
        Style::Link => &tags.link,
        Style::Dim => &tags.dim,
        Style::Heading(1) => &tags.h1,
        Style::Heading(2) => &tags.h2,
        Style::Heading(_) => &tags.h3,
        Style::Quote(1) => &tags.quote_1,
        Style::Quote(2) => &tags.quote_2,
        Style::Quote(_) => &tags.quote_3,
        Style::Table => &tags.table,
    }
}

/// Apply the GtkSourceView style scheme matching the libadwaita theme
/// ("Adwaita" for light, "Adwaita-dark" for dark).
pub fn apply_scheme(
    manager: &sourceview5::StyleSchemeManager,
    buffer: &sourceview5::Buffer,
    dark: bool,
) {
    let id = if dark { "Adwaita-dark" } else { "Adwaita" };
    if let Some(scheme) = manager.scheme(id) {
        buffer.set_style_scheme(Some(&scheme));
    }
}

/// Convert a `gdk::RGBA` to a `#rrggbb` hex string for tag properties.
fn rgba_to_hex(c: &gtk::gdk::RGBA) -> String {
    let r = (c.red() * 255.0).round() as u8;
    let g = (c.green() * 255.0).round() as u8;
    let b = (c.blue() * 255.0).round() as u8;
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Render Markdown into `buffer` using a small set of text tags. This is a
/// lightweight, dependency-free preview (no webview); it covers headings,
/// emphasis, code, lists, quotes, links, tables and rules.
pub fn render_markdown(buffer: &gtk::TextBuffer, tags: &PreviewTags, md: &str) {
    buffer.set_text("");
    let mut link_ranges = Vec::new();
    let mut end = buffer.end_iter();
    let rendered = markdown::render(md);
    for span in rendered.spans() {
        // `insert` invalidates `start`, so remember the offset and rebuild
        // the iterator afterwards instead of copying it (copying produced
        // a stale iter that made `apply_tag` fail with a Gtk-CRITICAL).
        let start_offset = end.offset();
        buffer.insert(&mut end, &span.text);
        if let Some(url) = &span.url {
            link_ranges.push((start_offset, end.offset(), url.clone()));
        }
        let start = buffer.iter_at_offset(start_offset);
        for style in &span.styles {
            buffer.apply_tag(style_tag(tags, *style), &start, &end);
        }
    }
    tags.set_link_ranges(link_ranges);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::markdown::Span;

    fn rendered(md: &str) -> String {
        build_spans(md)
            .into_iter()
            .map(|s| s.text)
            .collect::<Vec<_>>()
            .join("")
    }

    /// Parse `md` into styled spans (pure; no GTK involved).
    fn build_spans(md: &str) -> Vec<Span> {
        markdown::render(md).spans().to_vec()
    }

    #[test]
    fn headings_and_paragraphs() {
        assert_eq!(rendered("# Title\n\nbody"), "Title\n\nbody");
        assert_eq!(rendered("a\n\nb"), "a\n\nb");
        assert_eq!(rendered("a\nb"), "a\nb");
    }

    #[test]
    fn no_leading_or_trailing_blank_lines() {
        assert_eq!(rendered("# H"), "H");
        assert_eq!(rendered("- a"), "• a");
        assert_eq!(rendered("> q"), "q");
    }

    #[test]
    fn list_items_stay_on_their_bullet_line() {
        // Regression: item paragraphs used to push the text onto its own
        // line with a blank line after the bullet.
        assert_eq!(rendered("intro\n\n- a\n- b"), "intro\n\n• a\n• b");
        assert_eq!(rendered("- a\n\n- b"), "• a\n• b");
    }

    #[test]
    fn ordered_lists_keep_numbers() {
        assert_eq!(rendered("3. x\n4. y"), "3. x\n4. y");
    }

    #[test]
    fn nested_lists_indent() {
        assert_eq!(
            rendered("- a\n  - b\n    - c\n- d"),
            "• a\n  ◦ b\n    ▪ c\n• d"
        );
    }

    #[test]
    fn task_lists_show_checkboxes_instead_of_bullets() {
        assert_eq!(rendered("- [ ] a\n- [x] b"), "[ ] a\n[x] b");
    }

    #[test]
    fn code_blocks_are_trimmed_and_separated() {
        assert_eq!(rendered("```\nfn main() {}\n```"), "fn main() {}");
        assert_eq!(
            rendered("p\n\n```rust\nlet x = 1;\n```"),
            "p\n\nrust\nlet x = 1;"
        );
    }

    #[test]
    fn code_blocks_show_language_label() {
        assert_eq!(rendered("```rust\nlet x = 1;\n```"), "rust\nlet x = 1;");
        assert_eq!(rendered("``` python\nprint(1)\n```"), "python\nprint(1)");
        // Empty/whitespace-only info string: no label.
        assert_eq!(rendered("```  \nplain\n```"), "plain");
    }

    #[test]
    fn quotes_and_nested_quotes() {
        assert_eq!(rendered("> a\n> b"), "a\nb");
        assert_eq!(rendered("> a\n> > b"), "a\nb");
        assert_eq!(rendered("> a\n\n> b"), "a\n\nb");
    }

    #[test]
    fn rules_get_their_own_line() {
        assert_eq!(
            rendered("a\n\n---\n\nb"),
            format!("a\n\n{}\n\nb", markdown::RULE_LINE)
        );
    }

    #[test]
    fn tables_align_columns() {
        let out = rendered("| a | right |\n|:--|------:|\n| x | 1 |\n");
        assert!(out.contains("a │ right"), "{out}");
        assert!(out.contains("──┼─────"), "{out}");
        assert!(out.contains("x │     1"), "{out}");
    }

    #[test]
    fn tables_count_wide_chars_correctly() {
        let out = rendered("| 名称 | v |\n|---|---|\n| x | 1 |\n");
        assert!(out.contains("名称 │ v"), "{out}");
        assert!(out.contains("x    │ 1"), "{out}");
    }

    #[test]
    fn links_show_only_their_text() {
        assert_eq!(rendered("[text](https://x.org)"), "text");
        assert_eq!(rendered("<https://x.org>"), "https://x.org");
        assert_eq!(rendered("[x](x)"), "x");
    }

    #[test]
    fn link_spans_carry_destination() {
        let spans = build_spans("plain [text](https://x.org)");
        let link = spans
            .iter()
            .find(|s| s.styles.contains(&Style::Link))
            .expect("link span");
        assert_eq!(link.url.as_deref(), Some("https://x.org"));
        assert_eq!(
            spans
                .iter()
                .find(|s| s.text == "plain")
                .and_then(|s| s.url.as_deref()),
            None
        );
    }

    #[test]
    fn link_style_applied_to_link_text() {
        let spans = build_spans("[text](https://x.org)");
        assert!(
            spans
                .iter()
                .any(|s| s.text == "text" && s.styles.contains(&Style::Link))
        );
    }

    #[test]
    fn inline_styles_carry_context() {
        let spans = build_spans("**b** and `c`");
        assert!(
            spans
                .iter()
                .any(|s| s.text == "b" && s.styles.contains(&Style::Bold))
        );
        assert!(
            spans
                .iter()
                .any(|s| s.text == "c" && s.styles.contains(&Style::Code))
        );

        let spans = build_spans("# Head with `code`");
        assert!(spans.iter().any(|s| s.text == "code"
            && s.styles.contains(&Style::Code)
            && s.styles.contains(&Style::Heading(1))));
    }

    #[test]
    fn blocks_after_lists_quotes_and_code_separate() {
        assert_eq!(rendered("- a\n\npara"), "• a\n\npara");
        assert_eq!(rendered("> q\n\npara"), "q\n\npara");
        assert_eq!(rendered("```\nc\n```\n\npara"), "c\n\npara");
    }

    #[test]
    fn continuation_paragraph_inside_item() {
        // A second paragraph of the parent item stays attached to the item.
        assert_eq!(rendered("- a\n  - b\n\n  para2"), "• a\n  ◦ b\npara2");
    }

    /// Manual visual check: `cargo test --bin notas preview_screenshot -- --ignored --nocapture`
    /// then screenshot the window from outside.
    #[test]
    #[ignore = "manual probe: needs a display and visual inspection"]
    fn preview_screenshot() {
        gtk::init().expect("gtk init");
        let buffer = gtk::TextBuffer::new(None);
        let tags = PreviewTags::new(&buffer);
        let view = gtk::TextView::new();
        let css = gtk::CssProvider::new();
        css.load_from_string("textview { background: #123456; }");
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().expect("display"),
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        view.set_buffer(Some(&buffer));
        view.set_editable(false);
        view.set_wrap_mode(gtk::WrapMode::WordChar);
        view.set_left_margin(12);
        view.set_right_margin(12);
        view.set_top_margin(12);
        view.set_bottom_margin(12);
        let md = r#"# Heading One

## Heading Two

### Heading Three

#### Heading Four

Some paragraph with **bold**, *italic*, ~~strike~~, `inline code` and a
[link to example](https://example.com) plus a bare URL https://example.org here.

```rust
fn main() {
    println!("Hello, world!");
}
```

> A block quote line one.
> Line two.

> > Nested quote deeper.

- bullet one
- bullet two
  - nested bullet

1. first
2. second

---

| Name | Value |
|:-----|------:|
| a    |     1 |
| b    |    22 |
"#;
        render_markdown(&buffer, &tags, md);
        let window = gtk::Window::new();
        window.set_default_size(720, 900);
        window.set_child(Some(&view));
        window.present();
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(20) {
            glib::MainContext::default().iteration(true);
        }
    }
}
