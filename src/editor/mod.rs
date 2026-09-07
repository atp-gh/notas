//! The editor pane: GtkSourceView-based Markdown editor, a rendered
//! preview, and a find/replace bar.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita as adw;
use sourceview5::prelude::*;

#[cfg(test)]
use crate::markdown::Span;
use crate::markdown::{self, Style};
use crate::ui::protocol::AppMsg;

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
    fn new(buffer: &gtk::TextBuffer) -> Self {
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

pub struct Editor {
    pub source_view: sourceview5::View,
    pub source_buffer: sourceview5::Buffer,
    pub preview_buffer: gtk::TextBuffer,
    pub stack: gtk::Stack,
    pub search_bar: gtk::SearchBar,
    pub search_entry: gtk::SearchEntry,
    pub search_context: sourceview5::SearchContext,
    search_settings: sourceview5::SearchSettings,
    preview_tags: Rc<PreviewTags>,
}

impl Editor {
    pub fn render_preview(&self) {
        let md = self.source_buffer.text(
            &self.source_buffer.start_iter(),
            &self.source_buffer.end_iter(),
            true,
        );
        render_markdown(&self.preview_buffer, &self.preview_tags, &md);
    }

    pub fn set_search_text(&self, text: Option<&str>) {
        self.search_settings.set_search_text(text);
    }

    /// Move to the next match (from the current selection if any).
    pub fn find_next(&self) {
        let buffer = &self.source_buffer;
        let iter = if buffer.has_selection() {
            buffer.iter_at_mark(&buffer.selection_bound())
        } else {
            buffer.start_iter()
        };
        if let Some((start, end, _wrapped)) = self.search_context.forward(&iter) {
            buffer.select_range(&start, &end);
            let mut start = start;
            self.source_view
                .scroll_to_iter(&mut start, 0.0, false, 0.0, 0.0);
        }
    }

    pub fn find_prev(&self) {
        let buffer = &self.source_buffer;
        let iter = if buffer.has_selection() {
            buffer.iter_at_mark(&buffer.selection_bound())
        } else {
            buffer.start_iter()
        };
        if let Some((start, end, _wrapped)) = self.search_context.backward(&iter) {
            buffer.select_range(&start, &end);
            let mut start = start;
            self.source_view
                .scroll_to_iter(&mut start, 0.0, false, 0.0, 0.0);
        }
    }

    /// Replace every match of the current search text, returning the count.
    pub fn replace_all(&self, replacement: &str) -> usize {
        let buffer = &self.source_buffer;
        let mut iter = buffer.start_iter();
        let mut count = 0;
        while let Some((start, end, _wrapped)) = self.search_context.forward(&iter) {
            let mut s = start;
            let mut e = end;
            if self
                .search_context
                .replace(&mut s, &mut e, replacement)
                .is_ok()
            {
                count += 1;
            }
            iter = e;
        }
        count
    }
}

/// Build the whole editor pane. `emit` forwards UI events to the app
/// component; `suppress` gates the "changed" signal so that programmatic
/// loads don't mark the note dirty.
pub fn build_editor<E>(emit: E, suppress: Rc<Cell<bool>>) -> Editor
where
    E: Fn(AppMsg) + 'static + Clone,
{
    let source_buffer = sourceview5::Buffer::new(None);
    source_buffer.set_highlight_syntax(true);
    source_buffer.set_max_undo_levels(5000);

    // Markdown highlighting ships with GtkSourceView 5.
    let manager = sourceview5::LanguageManager::new();
    if let Some(lang) = manager.language("markdown") {
        source_buffer.set_language(Some(&lang));
    }

    {
        let emit = emit.clone();
        source_buffer.connect_changed(move |_| {
            if !suppress.get() {
                emit(AppMsg::ContentChanged);
            }
        });
    }

    let source_view = sourceview5::View::new();
    source_view.set_buffer(Some(&source_buffer));
    source_view.set_monospace(true);
    source_view.set_show_line_numbers(false);
    source_view.set_show_right_margin(false);
    source_view.set_wrap_mode(gtk::WrapMode::WordChar);

    let preview_buffer = gtk::TextBuffer::new(None);
    let preview_tags = Rc::new(PreviewTags::new(&preview_buffer));

    // Keep the preview colors and the syntax highlighting scheme in sync
    // with the libadwaita theme. GtkSourceView defaults to a light scheme
    // and does not switch it automatically, so apply it ourselves.
    let scheme_manager = sourceview5::StyleSchemeManager::new();
    {
        let style_manager = adw::StyleManager::default();
        let dark = style_manager.is_dark();
        let accent = style_manager.accent_color_rgba();
        preview_tags.apply_theme(dark, &accent);
        apply_scheme(&scheme_manager, &source_buffer, dark);

        let tags = preview_tags.clone();
        let sm = style_manager.clone();
        let buf = source_buffer.clone();
        sm.clone().connect_dark_notify(move |_| {
            let dark = sm.is_dark();
            tags.apply_theme(dark, &sm.accent_color_rgba());
            apply_scheme(&scheme_manager, &buf, dark);
        });

        let tags = preview_tags.clone();
        let sm = style_manager;
        sm.clone().connect_accent_color_rgba_notify(move |_| {
            tags.apply_theme(sm.is_dark(), &sm.accent_color_rgba());
        });
    }

    let preview_view = gtk::TextView::new();
    preview_view.set_buffer(Some(&preview_buffer));
    preview_view.set_editable(false);
    preview_view.set_cursor_visible(false);
    preview_view.set_wrap_mode(gtk::WrapMode::WordChar);
    preview_view.set_left_margin(8);
    preview_view.set_right_margin(8);
    preview_view.set_top_margin(8);
    preview_view.set_bottom_margin(8);

    // Clickable links: hover shows a pointer cursor, clicking opens the
    // URL in the system browser.
    {
        let tags = preview_tags.clone();
        let view = preview_view.clone();
        let gesture = gtk::GestureClick::new();
        gesture.connect_pressed(move |_g, _n, x, y| {
            if let Some(iter) = view.iter_at_location(x as i32, y as i32)
                && let Some(url) = tags.link_url_at(iter.offset())
            {
                let _ = gtk::gio::AppInfo::launch_default_for_uri(
                    &url,
                    None::<&gtk::gio::AppLaunchContext>,
                );
            }
        });
        preview_view.add_controller(gesture);

        let tags = preview_tags.clone();
        let view = preview_view.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |_m, x, y| {
            let over_link = view
                .iter_at_location(x as i32, y as i32)
                .is_some_and(|iter| tags.link_url_at(iter.offset()).is_some());
            let cursor = if over_link {
                gtk::gdk::Cursor::from_name("pointer", None)
            } else {
                None
            };
            view.set_cursor(cursor.as_ref());
        });
        preview_view.add_controller(motion);
    }

    // A TextView only gets a real viewport (and scrollbars) inside a
    // ScrolledWindow; without one the views grew to their full content
    // height and could not be scrolled.
    let source_scroll = gtk::ScrolledWindow::new();
    source_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    source_scroll.set_child(Some(&source_view));
    source_scroll.set_vexpand(true);

    let preview_scroll = gtk::ScrolledWindow::new();
    preview_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    preview_scroll.set_child(Some(&preview_view));
    preview_scroll.set_vexpand(true);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.add_named(&source_scroll, Some("source"));
    stack.add_named(&preview_scroll, Some("preview"));
    stack.set_visible_child_name("source");

    // --- find / replace -----------------------------------------------------
    let search_settings = sourceview5::SearchSettings::new();
    search_settings.set_wrap_around(true);
    let search_context = sourceview5::SearchContext::new(&source_buffer, Some(&search_settings));
    search_context.set_highlight(true);

    let search_entry = gtk::SearchEntry::new();
    search_entry.set_hexpand(true);
    search_entry.set_placeholder_text(Some(crate::tr!("Find…")));
    search_entry.set_width_chars(24);

    let replace_entry = gtk::Entry::new();
    replace_entry.set_placeholder_text(Some(crate::tr!("Replace with…")));
    replace_entry.set_width_chars(18);

    let replace_all_btn = gtk::Button::with_label(crate::tr!("Replace all"));
    replace_all_btn.set_tooltip_text(Some(crate::tr!("Replace every match")));

    let find_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    find_row.append(&search_entry);
    find_row.append(&replace_entry);
    find_row.append(&replace_all_btn);

    let search_bar = gtk::SearchBar::new();
    search_bar.set_child(Some(&find_row));
    search_bar.connect_entry(&search_entry);
    search_bar.set_key_capture_widget(Some(&source_view));

    {
        let emit = emit.clone();
        search_entry.connect_search_changed(move |entry| {
            let text = entry.text().to_string();
            emit(AppMsg::FindChanged(text));
        });
    }
    {
        let next = emit.clone();
        search_entry.connect_next_match(move |_| next(AppMsg::FindNext));
        let prev = emit.clone();
        search_entry.connect_previous_match(move |_| prev(AppMsg::FindPrev));
    }
    {
        replace_all_btn.connect_clicked(move |_| {
            let text = replace_entry.text().to_string();
            emit(AppMsg::ReplaceAll(text));
        });
    }

    Editor {
        source_view,
        source_buffer,
        preview_buffer,
        stack,
        search_bar,
        search_entry,
        search_context,
        search_settings,
        preview_tags,
    }
}

// ---------------------------------------------------------------------------
// Markdown -> styled spans
// ---------------------------------------------------------------------------

/// The bullet marker for a list item at the given nesting depth.
/// Parse `md` into styled spans (pure; no GTK involved).
#[cfg(test)]
fn build_spans(md: &str) -> Vec<Span> {
    markdown::render(md).spans().to_vec()
}

// ---------------------------------------------------------------------------
// Spans -> TextBuffer
// ---------------------------------------------------------------------------

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
fn apply_scheme(
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

    fn rendered(md: &str) -> String {
        build_spans(md)
            .into_iter()
            .map(|s| s.text)
            .collect::<Vec<_>>()
            .join("")
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

    /// Manual scrollability probe (run with `--ignored --nocapture`): prints
    /// the source view's vertical adjustment range inside a constrained
    /// window.
    #[test]
    #[ignore]
    fn source_view_scroll_probe() {
        gtk::init().expect("gtk init");
        adw::init().expect("adw init");
        let editor = build_editor(|_| {}, Rc::new(Cell::new(false)));
        let long = (0..300)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        editor.source_buffer.set_text(&long);

        let window = gtk::Window::new();
        window.set_default_size(600, 300);
        let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pane.append(&editor.stack);
        pane.set_vexpand(true);
        window.set_child(Some(&pane));
        window.present();
        let ctx = glib::MainContext::default();
        for _ in 0..50 {
            ctx.iteration(false);
        }

        let adj = editor
            .source_view
            .vadjustment()
            .expect("source vadjustment");
        let range = adj.upper() - adj.page_size();
        println!(
            "source view: upper={:.1} page={:.1} value={:.1} range={:.1}",
            adj.upper(),
            adj.page_size(),
            adj.value(),
            range
        );
        assert!(adj.page_size() > 100.0, "viewport must have real height");
        assert!(range > 0.0, "long content must be scrollable");
        adj.set_value(range);
        assert!((adj.value() - range).abs() < 1.0, "must scroll to the end");

        // Same for the preview: give it content, show it, and check its range.
        editor.render_preview();
        editor.stack.set_visible_child_name("preview");
        for _ in 0..50 {
            ctx.iteration(false);
        }
        let preview_view = editor
            .stack
            .child_by_name("preview")
            .and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok())
            .and_then(|s| s.child())
            .and_then(|w| w.downcast::<gtk::TextView>().ok())
            .expect("preview scrolled window");
        let preview_adj = preview_view.vadjustment().expect("preview vadjustment");
        let preview_range = preview_adj.upper() - preview_adj.page_size();
        println!(
            "preview view: upper={:.1} page={:.1} value={:.1} range={:.1}",
            preview_adj.upper(),
            preview_adj.page_size(),
            preview_adj.value(),
            preview_range
        );
        assert!(
            preview_adj.page_size() > 100.0,
            "preview viewport must have real height"
        );
        assert!(preview_range > 0.0, "long preview must be scrollable");
    }

    /// Manual check that the editor stays editable at the widget level:
    /// `cargo test --bin notas source_view_stays_editable -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn source_view_stays_editable() {
        gtk::init().expect("gtk init");
        adw::init().expect("adw init");
        let editor = build_editor(|_| {}, Rc::new(Cell::new(false)));
        assert!(
            editor.source_view.is_editable(),
            "source view must be editable"
        );
        assert!(
            editor.source_view.is_sensitive(),
            "source view must be sensitive"
        );
        assert_eq!(
            editor.stack.visible_child_name().as_deref(),
            Some("source"),
            "stack must show the source view by default"
        );
        // Typing inserts into the source buffer even with no note open.
        editor.source_buffer.insert_at_cursor("hello");
        let text = editor.source_buffer.text(
            &editor.source_buffer.start_iter(),
            &editor.source_buffer.end_iter(),
            true,
        );
        assert_eq!(text, "hello");

        // The syntax highlighting scheme follows the libadwaita theme.
        let dark = adw::StyleManager::default().is_dark();
        let expected = if dark { "Adwaita-dark" } else { "Adwaita" };
        let scheme = editor
            .source_buffer
            .style_scheme()
            .map(|s| s.id().to_string());
        assert_eq!(scheme.as_deref(), Some(expected));
    }

    /// Manual visual check: `cargo test --bin notas preview_screenshot -- --ignored --nocapture`
    /// then screenshot the window from outside.
    #[test]
    #[ignore]
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
