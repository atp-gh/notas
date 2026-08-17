//! The editor pane: GtkSourceView-based Markdown editor, a rendered
//! preview, and a find/replace bar.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
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
}

impl PreviewTags {
    fn new(buffer: &gtk::TextBuffer) -> Self {
        let h1 = buffer
            .create_tag(
                Some("h1"),
                &[
                    ("size-points", &22f64),
                    ("weight", &800i32),
                    ("underline", &pango::Underline::Single),
                    ("pixels-above-lines", &12i32),
                    ("pixels-below-lines", &4i32),
                ],
            )
            .expect("create h1 tag");
        let h2 = buffer
            .create_tag(
                Some("h2"),
                &[
                    ("size-points", &17f64),
                    ("weight", &700i32),
                    ("underline", &pango::Underline::Single),
                    ("pixels-above-lines", &10i32),
                    ("pixels-below-lines", &3i32),
                ],
            )
            .expect("create h2 tag");
        let h3 = buffer
            .create_tag(
                Some("h3"),
                &[
                    ("size-points", &14f64),
                    ("weight", &700i32),
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
                    ("background", &"rgba(127,127,127,0.15)"),
                ],
            )
            .expect("create code tag");
        let code_block = buffer
            .create_tag(
                Some("code-block"),
                &[
                    ("font", &"monospace"),
                    ("background", &"rgba(127,127,127,0.12)"),
                    ("left-margin", &12i32),
                    ("right-margin", &12i32),
                ],
            )
            .expect("create code-block tag");
        let quote_tag = |name: &str, margin: i32| {
            buffer
                .create_tag(
                    Some(name),
                    &[("foreground", &"#8f8f8f"), ("left-margin", &margin)],
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
        }
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
    preview_tags: PreviewTags,
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
    let preview_tags = PreviewTags::new(&preview_buffer);
    let preview_view = gtk::TextView::new();
    preview_view.set_buffer(Some(&preview_buffer));
    preview_view.set_editable(false);
    preview_view.set_cursor_visible(false);
    preview_view.set_wrap_mode(gtk::WrapMode::WordChar);
    preview_view.set_left_margin(8);
    preview_view.set_right_margin(8);
    preview_view.set_top_margin(8);
    preview_view.set_bottom_margin(8);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.add_named(&source_view, Some("source"));
    stack.add_named(&preview_view, Some("preview"));
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
    /// Span index at link start plus destination, for the "(url)" suffix.
    links: Vec<(usize, String)>,
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
}

impl Renderer {
    fn new() -> Self {
        Self {
            spans: Vec::new(),
            first: true,
            pending_sep: 0,
            inline: Vec::new(),
            links: Vec::new(),
            heading: 0,
            quote_depth: 0,
            lists: Vec::new(),
            item_prefix: None,
            item_indent: String::new(),
            item_blocks: Vec::new(),
            table: None,
            code_buf: None,
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
            });
            self.pending_sep = 0;
        }
        self.first = false;
        self.spans.push(Span {
            text: text.to_owned(),
            styles,
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
            Tag::CodeBlock(_) => {
                self.block_start();
                self.code_buf = Some(String::new());
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
                self.links.push((self.spans.len(), dest_url.into_string()));
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
                    if !code.is_empty() {
                        let mut styles = self.separator_styles();
                        styles.push(Style::CodeBlock);
                        self.emit(code, styles);
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
                if let Some((start, dest)) = self.links.pop() {
                    let visible: String = self.spans[start..]
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect();
                    if self.table.is_none()
                        && self.code_buf.is_none()
                        && !dest.is_empty()
                        && visible != dest
                    {
                        self.emit(&format!(" ({dest})"), vec![Style::Dim]);
                    }
                }
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

/// Render Markdown into `buffer` using a small set of text tags. This is a
/// lightweight, dependency-free preview (no webview); it covers headings,
/// emphasis, code, lists, quotes, links, tables and rules.
pub fn render_markdown(buffer: &gtk::TextBuffer, tags: &PreviewTags, md: &str) {
    buffer.set_text("");
    let mut end = buffer.end_iter();
    for span in build_spans(md) {
        let start = end;
        buffer.insert(&mut end, &span.text);
        for style in &span.styles {
            buffer.apply_tag(style_tag(tags, *style), &start, &end);
        }
    }
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
        assert_eq!(rendered("p\n\n```rust\nlet x = 1;\n```"), "p\n\nlet x = 1;");
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
    fn links_show_target_when_hidden() {
        assert_eq!(rendered("[text](https://x.org)"), "text (https://x.org)");
        assert_eq!(rendered("<https://x.org>"), "https://x.org");
        assert_eq!(rendered("[x](x)"), "x");
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
