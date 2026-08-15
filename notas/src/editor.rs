//! The editor pane: GtkSourceView-based Markdown editor, a rendered
//! preview, and a find/replace bar.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use sourceview5::prelude::*;

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
    pub quote: gtk::TextTag,
    pub link: gtk::TextTag,
}

impl PreviewTags {
    fn new(buffer: &gtk::TextBuffer) -> Self {
        let h1 = buffer
            .create_tag(
                Some("h1"),
                &[
                    ("size-points", &22f64),
                    ("weight", &800i32),
                    ("pixels-above-lines", &10i32),
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
                    ("pixels-above-lines", &8i32),
                    ("pixels-below-lines", &2i32),
                ],
            )
            .expect("create h2 tag");
        let h3 = buffer
            .create_tag(
                Some("h3"),
                &[
                    ("size-points", &14f64),
                    ("weight", &700i32),
                    ("pixels-above-lines", &6i32),
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
                &[("font", &"monospace"), ("background", &"rgba(127,127,127,0.15)")],
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
        let quote = buffer
            .create_tag(
                Some("quote"),
                &[
                    ("style", &pango::Style::Italic),
                    ("foreground", &"#8f8f8f"),
                    ("left-margin", &16i32),
                ],
            )
            .expect("create quote tag");
        let link = buffer
            .create_tag(
                Some("link"),
                &[
                    ("foreground", &"#3584e4"),
                    ("underline", &pango::Underline::Single),
                ],
            )
            .expect("create link tag");

        Self {
            h1,
            h2,
            h3,
            bold,
            italic,
            strike,
            code,
            code_block,
            quote,
            link,
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
// Markdown -> TextBuffer rendering
// ---------------------------------------------------------------------------

fn push_text(
    buffer: &gtk::TextBuffer,
    end: &mut gtk::TextIter,
    text: &str,
    tags: &[&gtk::TextTag],
) {
    let start = *end;
    buffer.insert(end, text);
    for tag in tags {
        buffer.apply_tag(*tag, &start, end);
    }
}

fn heading_tag(tags: &PreviewTags, level: u32) -> Option<&gtk::TextTag> {
    match level {
        1 => Some(&tags.h1),
        2 => Some(&tags.h2),
        _ if level >= 3 => Some(&tags.h3),
        _ => None,
    }
}

/// Render Markdown into `buffer` using a small set of text tags. This is a
/// lightweight, dependency-free preview (no webview); it covers headings,
/// emphasis, code, lists, quotes, links, tables and rules.
pub fn render_markdown(buffer: &gtk::TextBuffer, tags: &PreviewTags, md: &str) {
    buffer.set_text("");

    let mut end = buffer.end_iter();
    let mut inline: Vec<&gtk::TextTag> = Vec::new();
    let mut heading_level: u32 = 0;
    let mut list_counters: Vec<u64> = Vec::new();
    let mut first = true;

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES;

    for event in Parser::new_ext(md, options) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    heading_level = level as u32;
                    if !first {
                        push_text(buffer, &mut end, "\n\n", &[]);
                    }
                    first = false;
                }
                Tag::Strong => inline.push(&tags.bold),
                Tag::Emphasis => inline.push(&tags.italic),
                Tag::Strikethrough => inline.push(&tags.strike),
                Tag::Link { .. } => inline.push(&tags.link),
                Tag::CodeBlock(_) => {
                    if !first {
                        push_text(buffer, &mut end, "\n\n", &[]);
                    }
                    first = false;
                    inline.push(&tags.code_block);
                }
                Tag::BlockQuote(_) => {
                    if !first {
                        push_text(buffer, &mut end, "\n\n", &[]);
                    }
                    first = false;
                    inline.push(&tags.quote);
                }
                Tag::List(start) => {
                    list_counters.push(start.unwrap_or(0));
                }
                Tag::Item => {
                    let depth = list_counters.len();
                    let prefix = if depth == 0 {
                        "• ".to_string()
                    } else if list_counters[depth - 1] > 0 {
                        let n = list_counters[depth - 1];
                        list_counters[depth - 1] += 1;
                        format!("{n}. ")
                    } else {
                        "• ".to_string()
                    };
                    push_text(buffer, &mut end, &format!("\n{prefix}"), &[]);
                    first = false;
                }
                Tag::Paragraph => {
                    if !first {
                        push_text(buffer, &mut end, "\n\n", &[]);
                    }
                    first = false;
                }
                Tag::TableRow => {
                    push_text(buffer, &mut end, "\n", &[]);
                }
                Tag::TableCell => {
                    push_text(buffer, &mut end, "\t", &[]);
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => heading_level = 0,
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link => {
                    inline.pop();
                }
                TagEnd::CodeBlock => {
                    inline.pop();
                }
                TagEnd::BlockQuote(_) => {
                    inline.pop();
                }
                TagEnd::List(_) => {
                    list_counters.pop();
                }
                _ => {}
            },
            Event::Text(text) => {
                let mut tags_to_apply: Vec<&gtk::TextTag> = inline.clone();
                if let Some(h) = heading_tag(tags, heading_level) {
                    tags_to_apply.push(h);
                }
                push_text(buffer, &mut end, &text, &tags_to_apply);
                first = false;
            }
            Event::Code(text) => {
                push_text(buffer, &mut end, &text, &[&tags.code]);
                first = false;
            }
            Event::SoftBreak | Event::HardBreak => {
                push_text(buffer, &mut end, "\n", &[]);
            }
            Event::Rule => {
                push_text(buffer, &mut end, "────────────\n", &[]);
            }
            Event::TaskListMarker(checked) => {
                push_text(
                    buffer,
                    &mut end,
                    if checked { "[x] " } else { "[ ] " },
                    &[],
                );
            }
            _ => {}
        }
    }
}
