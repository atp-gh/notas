//! Markdown preview rendering: the text-tag palette plus the renderer that
//! turns frontend-neutral spans into a `gtk::TextBuffer`.
//!
//! This is the half of the editor pane that owns no widgets of its own
//! beyond the preview buffer's text tags. The `Editor` component and the
//! source/find UI live in `super`, which consumes `PreviewTags` and
//! [`apply_rendered`] (rendering itself runs on background threads via
//! [`markdown::render_themed`]).

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;
use sourceview5::prelude::*;

use crate::markdown::{self, Rgb, Style};

/// Cache key for syntax token tags: `(r, g, b, bold, italic)`.
type HlKey = (u8, u8, u8, bool, bool);

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
    /// Cache of per-token syntax tags keyed by `(r, g, b, bold, italic)`.
    hl_cache: RefCell<HashMap<HlKey, gtk::TextTag>>,
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
        // CONTEXT: `create_tag` only returns `None` for unknown property
        // names; all props here are static GTK `TextTag` props, so failure
        // is a programming error, not a runtime condition.
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
                .unwrap_or_else(|| unreachable!("create {name} tag"))
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
            hl_cache: RefCell::new(HashMap::new()),
            link_ranges: RefCell::new(Vec::new()),
        }
    }

    /// Fetch (creating on first use) the tag for a syntax token.
    pub fn syntax_tag(
        &self,
        buffer: &gtk::TextBuffer,
        fg: Rgb,
        bold: bool,
        italic: bool,
    ) -> gtk::TextTag {
        let key = (fg.0, fg.1, fg.2, bold, italic);
        if let Some(tag) = self.hl_cache.borrow().get(&key) {
            return tag.clone();
        }
        let hex = format!("#{:02x}{:02x}{:02x}", fg.0, fg.1, fg.2);
        let name = format!(
            "hl-{:02x}{:02x}{:02x}-{}-{}",
            fg.0,
            fg.1,
            fg.2,
            u8::from(bold),
            u8::from(italic)
        );
        let weight = if bold { 700i32 } else { 400i32 };
        let style = if italic {
            pango::Style::Italic
        } else {
            pango::Style::Normal
        };
        let tag = buffer
            .create_tag(
                Some(&name),
                &[("foreground", &hex), ("weight", &weight), ("style", &style)],
            )
            .unwrap_or_else(|| unreachable!("create {name} tag"));
        self.hl_cache.borrow_mut().insert(key, tag.clone());
        tag
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
    ///
    /// Muted text (`dim`/`strike`/quotes) is sampled from the current GTK
    /// theme (`view_fg_color` + `dim` style class) so any system theme
    /// (including Stylix-generated ones) is honored. When sampling fails
    /// (e.g. widget not rooted yet) it falls back to the previous
    /// dark/light constants. Code/quote backgrounds stay translucent
    /// overlays so they tint whatever `view_bg_color` the theme provides.
    pub fn apply_theme(&self, view: &gtk::TextView, dark: bool, accent: &gtk::gdk::RGBA) {
        let accent_hex = rgba_to_hex(accent);
        let text_hex = theme_muted_fg(view)
            .unwrap_or_else(|| (if dark { "#c8c8c8" } else { "#8f8f8f" }).to_string());
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
            q.set_property("foreground", text_hex.as_str());
            q.set_property("background", quote_bg);
        }
        self.dim.set_property("foreground", text_hex.as_str());
        self.strike.set_property("foreground", text_hex.as_str());
        self.code.set_property("background", code_bg);
        self.code_block.set_property("background", code_bg);
    }
}

/// The text tag that renders a given Markdown style.
///
/// Returns `None` for [`Style::Syntax`], which needs a per-token tag from
/// [`PreviewTags::syntax_tag`], and for [`Style::Mermaid`], which is embedded
/// as a diagram widget at its placeholder instead of styled text.
fn style_tag(tags: &PreviewTags, style: Style) -> Option<&gtk::TextTag> {
    match style {
        Style::Bold => Some(&tags.bold),
        Style::Italic => Some(&tags.italic),
        Style::Strike => Some(&tags.strike),
        Style::Code => Some(&tags.code),
        Style::CodeBlock => Some(&tags.code_block),
        Style::Syntax { .. } | Style::Mermaid => None,
        Style::Link => Some(&tags.link),
        Style::Dim => Some(&tags.dim),
        Style::Heading(1) => Some(&tags.h1),
        Style::Heading(2) => Some(&tags.h2),
        Style::Heading(_) => Some(&tags.h3),
        Style::Quote(1) => Some(&tags.quote_1),
        Style::Quote(2) => Some(&tags.quote_2),
        Style::Quote(_) => Some(&tags.quote_3),
        Style::Table => Some(&tags.table),
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

/// Sample a muted foreground from the current GTK theme.
///
/// Blends `view_fg_color` toward `view_bg_color` so quotes/`dim`/strike
/// stay readable on any system theme. Returns `None` when the widget has
/// no theme resolved yet (caller falls back to dark/light constants).
fn theme_muted_fg(view: &gtk::TextView) -> Option<String> {
    // CONTEXT: `style_context().lookup_color` is deprecated in gtk4 v4_22
    // with no drop-in replacement for theme color lookup; keep until the
    // GTK theme API migration lands.
    #[expect(deprecated, reason = "no non-deprecated theme color lookup yet")]
    let fg = view.style_context().lookup_color("view_fg_color")?;
    #[expect(deprecated, reason = "no non-deprecated theme color lookup yet")]
    let bg = view
        .style_context()
        .lookup_color("view_bg_color")
        .unwrap_or(gtk::gdk::RGBA::WHITE);
    // 65% fg + 35% bg approximates the old #8f8f8f/#c8c8c8 dimming
    // while tracking any theme palette.
    let mix = |f: f32, b: f32| f * 0.65 + b * 0.35;
    let muted = gtk::gdk::RGBA::new(
        mix(fg.red(), bg.red()),
        mix(fg.green(), bg.green()),
        mix(fg.blue(), bg.blue()),
        1.0,
    );
    Some(rgba_to_hex(&muted))
}

/// Convert a `gdk::RGBA` to a `#rrggbb` hex string for tag properties.
fn rgba_to_hex(c: &gtk::gdk::RGBA) -> String {
    let r = (c.red() * 255.0).round() as u8;
    let g = (c.green() * 255.0).round() as u8;
    let b = (c.blue() * 255.0).round() as u8;
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Apply an already-rendered document to `buffer` using a small set of
/// text tags. This is a lightweight preview (no webview); it covers
/// headings, emphasis, code (with syntax highlighting), lists, quotes,
/// links, tables, rules and Mermaid diagrams (embedded as `gtk::Picture`
/// widgets at their placeholders via `view`).
///
/// Split out so the expensive [`markdown::render_themed`] step can run on a
/// background thread while only this cheap bulk insert + tag pass runs on
/// the UI thread. `rendered` is plain `String` data (`Send`), safe to move
/// across threads.
pub fn apply_rendered(
    view: &gtk::TextView,
    buffer: &gtk::TextBuffer,
    tags: &PreviewTags,
    rendered: &markdown::RenderedMarkdown,
) {
    // Single bulk insert: per-span `insert` made 300-line blocks do
    // thousands of signal emissions + layout passes (seconds of freeze).
    let full: String = rendered.spans().iter().map(|s| s.text.as_str()).collect();
    // `set_text` drops previous child anchors (and unparents their diagram
    // widgets), so anchors are rebuilt below on every render.
    buffer.set_text(&full);
    let mut link_ranges = Vec::new();
    // Diagram anchors to insert after the tag pass, as pre-insertion char
    // offsets. Inserting a child anchor mutates the buffer and shifts every
    // `TextIter` at or after it, so anchors must not be created mid-walk:
    // with two diagrams the second anchor landed on a stale iter and only
    // the first diagram ever showed.
    let mut pending_diagrams: Vec<(i32, &str)> = Vec::new();
    // Walk forward once; `forward_chars` keeps this O(n) instead of
    // O(n²) `iter_at_offset` rescans.
    let mut iter = buffer.start_iter();
    for span in rendered.spans() {
        let start = iter;
        let chars = span.text.chars().count() as i32;
        let mut end = start;
        end.forward_chars(chars);
        if let Some(url) = &span.url {
            link_ranges.push((start.offset(), end.offset(), url.clone()));
        }
        for style in &span.styles {
            match *style {
                Style::Syntax { fg, bold, italic } => {
                    let tag = tags.syntax_tag(buffer, fg, bold, italic);
                    buffer.apply_tag(&tag, &start, &end);
                }
                other => {
                    if let Some(tag) = style_tag(tags, other) {
                        buffer.apply_tag(tag, &start, &end);
                    }
                }
            }
        }
        if let Some(svg) = span.mermaid_svg.as_deref() {
            pending_diagrams.push((start.offset(), svg));
        }
        iter = end;
    }
    // Insert back-to-front with fresh iters so earlier offsets stay valid.
    // Diagram count per note is tiny, so one `iter_at_offset` each is fine.
    for (offset, svg) in pending_diagrams.iter().rev() {
        let mut anchor_iter = buffer.iter_at_offset(*offset);
        embed_mermaid_at(view, buffer, &mut anchor_iter, svg);
    }
    tags.set_link_ranges(shift_ranges_for_anchors(
        link_ranges,
        pending_diagrams.iter().map(|(offset, _)| *offset),
    ));
}

/// Create the diagram widget at an already-positioned anchor iter.
fn embed_mermaid_at(
    view: &gtk::TextView,
    buffer: &gtk::TextBuffer,
    anchor_iter: &mut gtk::TextIter,
    svg: &str,
) {
    let bytes = glib::Bytes::from(svg.as_bytes());
    let paintable = gtk::Svg::from_bytes(&bytes);
    let picture = gtk::Picture::for_paintable(&paintable);
    picture.set_content_fit(gtk::ContentFit::ScaleDown);
    picture.set_can_shrink(true);
    // A TextView anchor allocates the widget at its requested size; without
    // one an SVG whose dimensions `gtk::Svg` cannot infer collapses to zero.
    // `render_mermaid` already emits fixed dimensions, this is the backstop.
    let (width, height) = mermaid_svg_size(svg);
    picture.set_size_request(width, height);
    let anchor = buffer.create_child_anchor(anchor_iter);
    view.add_child_at_anchor(&picture, &anchor);
}

/// Shift link ranges past inserted diagram anchors.
///
/// Each anchor occupies one char at its insertion offset, pushing content at
/// or after it one position forward. Pure and testable without GTK.
fn shift_ranges_for_anchors(
    ranges: Vec<(i32, i32, String)>,
    anchors: impl Iterator<Item = i32>,
) -> Vec<(i32, i32, String)> {
    let offsets: Vec<i32> = anchors.collect();
    ranges
        .into_iter()
        .map(|(start, end, url)| {
            let shift = |pos: i32| pos + offsets.iter().filter(|o| **o <= pos).count() as i32;
            (shift(start), shift(end), url)
        })
        .collect()
}

/// Natural size of an SVG document, capped to a sane preview width.
///
/// Parses the root `width="W" height="H"` attributes (emitted by
/// `render_mermaid` with `responsive = false`); falls back to the `viewBox`
/// dimensions, then to a 480x320 default. Pure and testable without GTK.
fn mermaid_svg_size(svg: &str) -> (i32, i32) {
    const FALLBACK: (i32, i32) = (480, 320);
    let root = svg.split('>').next().unwrap_or("");
    if let Some(w) = parse_svg_number(root, "width")
        && let Some(h) = parse_svg_number(root, "height")
    {
        return cap_size(w, h);
    }
    if let Some(view_box) = parse_svg_attr_str(root, "viewBox")
        && let Some((w, h)) = parse_view_box(&view_box)
    {
        return cap_size(w, h);
    }
    FALLBACK
}

/// Scale `(w, h)` down to [`MAX`](mermaid_svg_size#MAX_WIDTH) preview width,
/// rounding up to at least 1px per side.
fn cap_size(w: f64, h: f64) -> (i32, i32) {
    const MAX_WIDTH: f64 = 720.0;
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return (480, 320);
    }
    let scale = (MAX_WIDTH / w).min(1.0);
    // `ceil` avoids a 0px side for hairline diagrams; clamp guards the cast.
    let width = (w * scale).ceil().clamp(1.0, MAX_WIDTH) as i32;
    let height = (h * scale).ceil().clamp(1.0, 4096.0) as i32;
    (width, height)
}

/// Parse a numeric `name="..."` attribute from an SVG root tag.
///
/// Accepts a trailing `px` (ignored); rejects percentages.
fn parse_svg_number(tag: &str, name: &str) -> Option<f64> {
    let value = parse_svg_attr_str(tag, name)?;
    if value.contains('%') {
        return None;
    }
    value.trim().trim_end_matches("px").trim().parse().ok()
}

/// Extract the raw `name="..."` value from an SVG root tag.
fn parse_svg_attr_str(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = tag.find(needle.as_str())? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_owned())
}

/// Parse a `viewBox="minx miny w h"` value into `(w, h)`.
fn parse_view_box(value: &str) -> Option<(f64, f64)> {
    let parts: Vec<f64> = value
        .split([',', ' ', '\t', '\n'])
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    if parts.len() == 4 && parts[2] > 0.0 && parts[3] > 0.0 {
        Some((parts[2], parts[3]))
    } else {
        None
    }
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

    #[test]
    fn mermaid_size_prefers_fixed_dimensions() {
        assert_eq!(
            mermaid_svg_size(r#"<svg width="108" height="216" viewBox="0 0 108 216">"#),
            (108, 216)
        );
    }

    #[test]
    fn mermaid_size_falls_back_to_view_box() {
        assert_eq!(
            mermaid_svg_size(r#"<svg width="100%" viewBox="0 0 108 216">"#),
            (108, 216)
        );
    }

    #[test]
    fn mermaid_size_caps_wide_diagrams() {
        assert_eq!(
            mermaid_svg_size(r#"<svg width="1440" height="100" viewBox="0 0 1440 100">"#),
            (720, 50)
        );
    }

    #[test]
    fn mermaid_size_rejects_garbage() {
        assert_eq!(mermaid_svg_size("<svg>"), (480, 320));
    }

    #[test]
    fn anchor_shifts_a_range_after_it() {
        let shifted = shift_ranges_for_anchors(vec![(10, 14, "u".into())], [4].into_iter());
        assert_eq!(shifted, vec![(11, 15, "u".into())]);
    }

    #[test]
    fn anchor_leaves_a_range_before_it() {
        let shifted = shift_ranges_for_anchors(vec![(2, 4, "u".into())], [10].into_iter());
        assert_eq!(shifted, vec![(2, 4, "u".into())]);
    }

    #[test]
    fn two_anchors_shift_a_later_range_twice() {
        let shifted = shift_ranges_for_anchors(vec![(20, 24, "u".into())], [4, 12].into_iter());
        assert_eq!(shifted, vec![(22, 26, "u".into())]);
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
        let rendered = markdown::render_themed(md, false);
        apply_rendered(&view, &buffer, &tags, &rendered);
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
