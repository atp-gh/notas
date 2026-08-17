//! The editor pane: GtkSourceView-based Markdown editor, a rendered
//! preview, and a find/replace bar.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use libadwaita as adw;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use sourceview5::prelude::*;
use unicode_width::UnicodeWidthStr;

use crate::app::AppMsg;

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
        let quote_bg = if dark { "rgba(255,255,255,0.06)" } else { "rgba(0,0,0,0.05)" };
        let code_bg = if dark { "rgba(255,255,255,0.10)" } else { "rgba(127,127,127,0.18)" };

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
    pub preview_view: gtk::TextView,
    pub preview_buffer: gtk::TextBuffer,
    pub stack: gtk::Stack,
    pub search_bar: gtk::SearchBar,
    pub search_entry: gtk::SearchEntry,
    pub replace_entry: gtk::Entry,
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
        loop {
            match self.search_context.forward(&iter) {
                Some((start, end, _wrapped)) => {
                    let mut s = start;
                    let mut e = end;
                    if self.search_context.replace(&mut s, &mut e, replacement).is_ok() {
                        count += 1;
                    }
                    iter = e;
                }
                None => break,
            }
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
        let suppress = suppress.clone();
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

    // Keep the preview colors in sync with the libadwaita theme.
    {
        let style_manager = adw::StyleManager::default();
        let accent = style_manager.accent_color_rgba();
        preview_tags.apply_theme(style_manager.is_dark(), &accent);

        let tags = preview_tags.clone();
        let sm = style_manager.clone();
        sm.clone().connect_dark_notify(move |_| {
            let accent = sm.accent_color_rgba();
            tags.apply_theme(sm.is_dark(), &accent);
        });

        let tags = preview_tags.clone();
        let sm = style_manager.clone();
        sm.clone().connect_accent_color_rgba_notify(move |_| {
            let accent = sm.accent_color_rgba();
            tags.apply_theme(sm.is_dark(), &accent);
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
            if let Some(iter) = view.iter_at_location(x as i32, y as i32) {
                if let Some(url) = tags.link_url_at(iter.offset()) {
                    let _ = gtk::gio::AppInfo::launch_default_for_uri(
                        &url,
                        None::<&gtk::gio::AppLaunchContext>,
                    );
                }
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
        let emit = emit.clone();
        let replace_entry = replace_entry.clone();
        replace_all_btn.connect_clicked(move |_| {
            let text = replace_entry.text().to_string();
            emit(AppMsg::ReplaceAll(text));
        });
    }

    Editor {
        source_view,
        source_buffer,
        preview_view,
        preview_buffer,
        stack,
        search_bar,
        search_entry,
        replace_entry,
        search_context,
        search_settings,
        preview_tags,
    }
}

// ---------------------------------------------------------------------------
// Markdown -> styled spans
// ---------------------------------------------------------------------------

/// A visual style that maps to one `gtk::TextTag` in the preview.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Style {
    Bold,
    Italic,
    Strike,
    Code,
    CodeBlock,
    Link,
    Dim,
    Heading(u32),
    Quote(u32),
    Table,
}

/// A run of text carrying the styles active at that position.
struct Span {
    text: String,
    styles: Vec<Style>,
    /// Destination URL when this span sits inside a link, else `None`.
    url: Option<String>,
}

/// Horizontal rule drawn in the preview.
const RULE_LINE: &str = "────────────────────────────────────────";

/// Per-list state: ordered counter or bullets, and whether any item rendered.
struct ListState {
    /// `Some(n)` for ordered lists (next number to emit), `None` for bullets.
    next: Option<u64>,
    emitted_any: bool,
}

/// Cells accumulated while a table is being parsed.
struct TableState {
    alignments: Vec<pulldown_cmark::Alignment>,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
}

/// Turns Markdown events into styled spans. All block separation flows
/// through `sep`/`emit`, so newlines are emitted exactly once, right before
/// the content they separate.
struct Renderer {
    spans: Vec<Span>,
    /// True until the first content is emitted; suppresses leading newlines.
    first: bool,
    /// Newlines owed before the next content (0–2).
    pending_sep: usize,
    inline: Vec<Style>,
    /// Destination of the link currently being parsed, if any.
    link_url: Option<String>,
    heading: u32,
    quote_depth: u32,
    lists: Vec<ListState>,
    /// Bullet/number prefix waiting to be emitted with the item's first text.
    item_prefix: Option<String>,
    /// Indent of the current item, reused for task-list checkboxes.
    item_indent: String,
    /// Blocks seen inside each open list item, innermost last.
    item_blocks: Vec<u32>,
    table: Option<TableState>,
    code_buf: Option<String>,
    /// Language from the code fence (first word of the info string).
    code_lang: Option<String>,
}

impl Renderer {
    fn new() -> Self {
        Self {
            spans: Vec::new(),
            first: true,
            pending_sep: 0,
            inline: Vec::new(),
            link_url: None,
            heading: 0,
            quote_depth: 0,
            lists: Vec::new(),
            item_prefix: None,
            item_indent: String::new(),
            item_blocks: Vec::new(),
            table: None,
            code_buf: None,
            code_lang: None,
        }
    }

    /// Declare separation before a block: a blank line at top level, a
    /// single newline between blocks of one list item. The first block of
    /// an item flows directly after the bullet.
    fn block_start(&mut self) {
        match self.item_blocks.last().copied() {
            Some(count) => {
                if count > 0 {
                    self.sep(1);
                }
                if let Some(blocks) = self.item_blocks.last_mut() {
                    *blocks += 1;
                }
            }
            None => {
                if self.pending_sep == 0 {
                    self.sep(2);
                }
            }
        }
    }

    fn sep(&mut self, n: usize) {
        if !self.first {
            self.pending_sep = self.pending_sep.max(n);
        }
    }

    /// Styles for newline-only spans so blank lines keep the quote indent.
    fn separator_styles(&self) -> Vec<Style> {
        if self.quote_depth > 0 {
            vec![Style::Quote(self.quote_depth)]
        } else {
            Vec::new()
        }
    }

    /// Emit content, flushing any owed block separation first.
    fn emit(&mut self, text: &str, styles: Vec<Style>) {
        if text.is_empty() {
            return;
        }
        if self.pending_sep > 0 {
            let styles = self.separator_styles();
            self.spans.push(Span {
                text: "\n".repeat(self.pending_sep),
                styles,
                url: None,
            });
            self.pending_sep = 0;
        }
        self.first = false;
        self.spans.push(Span {
            text: text.to_owned(),
            styles,
            url: self.link_url.clone(),
        });
    }

    /// Styles active at the current position: inline stack plus heading
    /// and quote context.
    fn context_styles(&self) -> Vec<Style> {
        let mut styles = self.inline.clone();
        if self.heading > 0 {
            styles.push(Style::Heading(self.heading));
        }
        if self.quote_depth > 0 {
            styles.push(Style::Quote(self.quote_depth));
        }
        styles
    }

    /// Emit the pending item prefix (bullet/number), if any.
    fn flush_prefix(&mut self) {
        if let Some(prefix) = self.item_prefix.take() {
            let styles = self.separator_styles();
            self.emit(&prefix, styles);
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(text);
            return;
        }
        if let Some(buf) = self.code_buf.as_mut() {
            buf.push_str(text);
            return;
        }
        self.flush_prefix();
        let styles = self.context_styles();
        self.emit(text, styles);
    }

    fn code(&mut self, code: &str) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(code);
            return;
        }
        self.flush_prefix();
        let mut styles = self.context_styles();
        styles.push(Style::Code);
        self.emit(code, styles);
    }

    fn line_break(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push(' ');
            return;
        }
        let styles = self.separator_styles();
        self.emit("\n", styles);
    }

    fn task_marker(&mut self, checked: bool) {
        let marker = if checked { "[x] " } else { "[ ] " };
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(marker);
            return;
        }
        // Task items show a checkbox instead of the bullet.
        if self.item_prefix.is_some() {
            self.item_prefix = None;
            let prefix = format!("{}{}", self.item_indent, marker);
            let styles = self.separator_styles();
            self.emit(&prefix, styles);
        } else {
            self.emit(marker, Vec::new());
        }
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.block_start(),
            Tag::Heading { level, .. } => {
                self.block_start();
                self.heading = level as u32;
            }
            Tag::BlockQuote(_) => {
                self.quote_depth += 1;
                if self.quote_depth == 1 {
                    self.block_start();
                } else {
                    self.sep(1);
                }
            }
            Tag::CodeBlock(kind) => {
                self.block_start();
                self.code_buf = Some(String::new());
                self.code_lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let lang = info
                            .trim()
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if lang.is_empty() {
                            None
                        } else {
                            Some(lang)
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
            }
            Tag::List(start) => {
                if let Some(blocks) = self.item_blocks.last_mut() {
                    *blocks += 1;
                }
                self.lists.push(ListState {
                    next: start,
                    emitted_any: false,
                });
            }
            Tag::Item => {
                let depth = self.lists.len();
                let first_item = self.lists.last().is_none_or(|l| !l.emitted_any);
                if first_item && depth <= 1 {
                    self.block_start();
                } else {
                    self.sep(1);
                }
                let marker = {
                    let list = self
                        .lists
                        .last_mut()
                        .expect("Item must follow a List start");
                    list.emitted_any = true;
                    match list.next {
                        Some(n) => {
                            list.next = Some(n + 1);
                            format!("{n}. ")
                        }
                        None => match depth {
                            1 => "• ".to_owned(),
                            2 => "◦ ".to_owned(),
                            _ => "▪ ".to_owned(),
                        },
                    }
                };
                self.item_indent = " ".repeat(2 * (depth - 1));
                self.item_prefix = Some(format!("{}{}", self.item_indent, marker));
                self.item_blocks.push(0);
            }
            Tag::Strong => self.inline.push(Style::Bold),
            Tag::Emphasis => self.inline.push(Style::Italic),
            Tag::Strikethrough => self.inline.push(Style::Strike),
            Tag::Link { dest_url, .. } => {
                self.link_url = Some(dest_url.into_string());
                self.inline.push(Style::Link);
            }
            Tag::Image { .. } => {}
            Tag::Table(alignments) => {
                self.block_start();
                self.table = Some(TableState {
                    alignments,
                    rows: Vec::new(),
                    row: Vec::new(),
                    cell: String::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.cell.clear();
                }
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {}
            TagEnd::Heading(_) => self.heading = 0,
            TagEnd::BlockQuote(_) => self.quote_depth = self.quote_depth.saturating_sub(1),
            TagEnd::CodeBlock => {
                if let Some(buf) = self.code_buf.take() {
                    let code = buf.trim_end_matches('\n');
                    let lang = self.code_lang.take();
                    if !code.is_empty() || lang.is_some() {
                        let mut styles = self.separator_styles();
                        styles.push(Style::CodeBlock);
                        if let Some(lang) = lang {
                            let mut lang_styles = styles.clone();
                            lang_styles.push(Style::Dim);
                            self.emit(&format!("{lang}\n"), lang_styles);
                        }
                        if !code.is_empty() {
                            self.emit(code, styles);
                        }
                    }
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item => {
                if self.item_blocks.last() == Some(&0) {
                    // Empty item: keep the lone bullet.
                    self.flush_prefix();
                }
                self.item_prefix = None;
                self.item_blocks.pop();
            }
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => {
                self.inline.pop();
            }
            TagEnd::Link => {
                self.inline.pop();
                self.link_url = None;
            }
            TagEnd::Table => self.render_table(),
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    let cell = std::mem::take(&mut table.cell);
                    table.row.push(cell);
                }
            }
            _ => {}
        }
    }

    /// Lay the buffered table out as aligned, padded rows.
    fn render_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        if table.rows.is_empty() {
            return;
        }

        let cols = table
            .alignments
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        let mut widths = vec![0usize; cols];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.width());
            }
        }

        let mut lines: Vec<(String, Vec<Style>)> = Vec::with_capacity(table.rows.len() + 1);
        for (row_index, row) in table.rows.iter().enumerate() {
            let cells: Vec<String> = (0..cols)
                .map(|i| {
                    pad_cell(
                        row.get(i).map(String::as_str).unwrap_or(""),
                        widths[i],
                        *table.alignments.get(i).unwrap_or(&Alignment::None),
                    )
                })
                .collect();
            if row_index == 0 {
                lines.push((cells.join(" │ "), vec![Style::Table, Style::Bold]));
                let rule = widths
                    .iter()
                    .map(|w| "─".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                lines.push((rule, vec![Style::Table, Style::Dim]));
            } else {
                lines.push((cells.join(" │ "), vec![Style::Table]));
            }
        }
        for (i, (text, styles)) in lines.into_iter().enumerate() {
            if i > 0 {
                self.sep(1);
            }
            self.emit(&text, styles);
        }
    }
}

/// Pad a table cell to its column width, honouring the column alignment.
fn pad_cell(cell: &str, width: usize, align: Alignment) -> String {
    let pad = width.saturating_sub(cell.width());
    match align {
        Alignment::Right => format!("{}{}", " ".repeat(pad), cell),
        Alignment::Center => {
            let left = pad / 2;
            format!("{}{}{}", " ".repeat(left), cell, " ".repeat(pad - left))
        }
        _ => format!("{}{}", cell, " ".repeat(pad)),
    }
}

/// Parse `md` into styled spans (pure; no GTK involved).
fn build_spans(md: &str) -> Vec<Span> {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let mut renderer = Renderer::new();
    for event in Parser::new_ext(md, options) {
        match event {
            Event::Start(tag) => renderer.start_tag(tag),
            Event::End(tag) => renderer.end_tag(tag),
            Event::Text(text) => renderer.text(&text),
            Event::Code(text) => renderer.code(&text),
            Event::SoftBreak | Event::HardBreak => renderer.line_break(),
            Event::Rule => {
                renderer.block_start();
                renderer.emit(RULE_LINE, vec![Style::Dim]);
            }
            Event::TaskListMarker(checked) => renderer.task_marker(checked),
            _ => {}
        }
    }
    renderer.spans
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
    for span in build_spans(md) {
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
        assert_eq!(rendered("a\n\n---\n\nb"), format!("a\n\n{RULE_LINE}\n\nb"));
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
            spans.iter().find(|s| s.text == "plain").and_then(|s| s.url.as_deref()),
            None
        );
    }

    #[test]
    fn link_style_applied_to_link_text() {
        let spans = build_spans("[text](https://x.org)");
        assert!(spans
            .iter()
            .any(|s| s.text == "text" && s.styles.contains(&Style::Link)));
    }

    #[test]
    fn inline_styles_carry_context() {
        let spans = build_spans("**b** and `c`");
        assert!(spans
            .iter()
            .any(|s| s.text == "b" && s.styles.contains(&Style::Bold)));
        assert!(spans
            .iter()
            .any(|s| s.text == "c" && s.styles.contains(&Style::Code)));

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

        let adj = editor.source_view.vadjustment().expect("source vadjustment");
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
        let preview_adj = editor.preview_view.vadjustment().expect("preview vadjustment");
        let preview_range = preview_adj.upper() - preview_adj.page_size();
        println!(
            "preview view: upper={:.1} page={:.1} value={:.1} range={:.1}",
            preview_adj.upper(),
            preview_adj.page_size(),
            preview_adj.value(),
            preview_range
        );
        assert!(preview_adj.page_size() > 100.0, "preview viewport must have real height");
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
        assert!(editor.source_view.is_editable(), "source view must be editable");
        assert!(editor.source_view.is_sensitive(), "source view must be sensitive");
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
        css.load_from_data("textview { background: #123456; }");
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
