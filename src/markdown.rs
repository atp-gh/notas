//! Platform-neutral Markdown rendering data.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use unicode_width::UnicodeWidthStr;

/// A visual style applied to a rendered Markdown span.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// Bold text.
    Bold,
    /// Italic text.
    Italic,
    /// Strikethrough text.
    Strike,
    /// Inline code.
    Code,
    /// Fenced code block.
    CodeBlock,
    /// Syntax-highlighted token inside a code block. The base
    /// [`Style::CodeBlock`] (monospace + background) is always applied
    /// first; this only carries the per-token foreground + emphasis.
    Syntax {
        /// Token foreground color.
        fg: Rgb,
        /// Whether the token is bold.
        bold: bool,
        /// Whether the token is italic.
        italic: bool,
    },
    /// Link text.
    Link,
    /// Muted metadata or separator.
    Dim,
    /// Heading level.
    Heading(u32),
    /// Block quote depth.
    Quote(u32),
    /// Table cell text.
    Table,
    /// Mermaid diagram placeholder. The span's text is
    /// [`MERMAID_PLACEHOLDER`]; the rendered SVG bytes live in
    /// [`Span::mermaid_svg`] so frontends can embed the diagram.
    Mermaid,
}

/// An RGB foreground color for syntax tokens (frontend-neutral).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A text run carrying the styles active at that position.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    /// Display text for the run.
    pub text: String,
    /// Styles active for the run.
    pub styles: Vec<Style>,
    /// Destination URL when the span is link text.
    pub url: Option<String>,
    /// Rendered Mermaid SVG for [`Style::Mermaid`] placeholder spans.
    /// `None` for every other span.
    pub mermaid_svg: Option<Box<str>>,
}

/// Platform-neutral result of rendering a Markdown document.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RenderedMarkdown {
    spans: Vec<Span>,
}

/// State for one open Markdown list while parsing.
#[derive(Debug, Clone, Copy)]
pub struct ListState {
    /// Next ordered-list number, or `None` for an unordered list.
    pub next: Option<u64>,
    /// Whether an item has already been rendered.
    pub emitted_any: bool,
}

/// Cells accumulated for one Markdown table while parsing.
#[derive(Debug, Default)]
pub struct TableState {
    /// Requested alignment for each column.
    pub alignments: Vec<pulldown_cmark::Alignment>,
    /// Completed rows.
    pub rows: Vec<Vec<String>>,
    /// Current row under construction.
    pub row: Vec<String>,
    /// Current cell under construction.
    pub cell: String,
}

/// Stateful Markdown event renderer shared by frontend adapters.
pub struct Renderer {
    /// Ordered rendered spans.
    pub spans: Vec<Span>,
    /// Whether any content has been emitted.
    pub first: bool,
    /// Pending block separator length.
    pub pending_sep: usize,
    /// Active inline styles.
    pub inline: Vec<Style>,
    /// Active link destination.
    pub link_url: Option<String>,
    /// Active heading level.
    pub heading: u32,
    /// Active block quote depth.
    pub quote_depth: u32,
    /// Open list states.
    pub lists: Vec<ListState>,
    /// Pending list item prefix.
    pub item_prefix: Option<String>,
    /// Current list item indentation.
    pub item_indent: String,
    /// Number of blocks seen in each open item.
    pub item_blocks: Vec<u32>,
    /// Active table state.
    pub table: Option<TableState>,
    /// Active fenced code buffer.
    pub code_buf: Option<String>,
    /// Optional fenced code language.
    pub code_lang: Option<String>,
    /// Whether to use the dark syntax theme for code blocks.
    pub dark: bool,
}

impl Renderer {
    /// Create an empty renderer state.
    #[must_use]
    pub fn new() -> Self {
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
            dark: false,
        }
    }

    /// Create a renderer that highlights code blocks for `dark` mode.
    #[must_use]
    pub fn with_dark(dark: bool) -> Self {
        let mut renderer = Self::new();
        renderer.dark = dark;
        renderer
    }

    /// Declare separation before the next block.
    pub fn block_start(&mut self) {
        match self.item_blocks.last().copied() {
            Some(count) => {
                if count > 0 {
                    self.sep(1);
                }
                if let Some(blocks) = self.item_blocks.last_mut() {
                    *blocks += 1;
                }
            }
            None if self.pending_sep == 0 => self.sep(2),
            None => {}
        }
    }

    /// Request at least `count` newlines before the next content.
    pub fn sep(&mut self, count: usize) {
        if !self.first {
            self.pending_sep = self.pending_sep.max(count);
        }
    }

    /// Styles for separator spans in the current quote context.
    #[must_use]
    pub fn separator_styles(&self) -> Vec<Style> {
        if self.quote_depth > 0 {
            vec![Style::Quote(self.quote_depth)]
        } else {
            Vec::new()
        }
    }

    /// Emit text with styles, flushing pending block separation first.
    pub fn emit(&mut self, text: &str, styles: Vec<Style>) {
        if text.is_empty() {
            return;
        }
        if self.pending_sep > 0 {
            self.spans.push(Span {
                text: "\n".repeat(self.pending_sep),
                styles: self.separator_styles(),
                url: None,
                mermaid_svg: None,
            });
            self.pending_sep = 0;
        }
        self.first = false;
        self.spans.push(Span {
            text: text.to_owned(),
            styles,
            url: self.link_url.clone(),
            mermaid_svg: None,
        });
    }

    /// Emit a Mermaid diagram placeholder carrying its rendered SVG.
    pub fn emit_mermaid(&mut self, svg: Box<str>) {
        if self.pending_sep > 0 {
            self.spans.push(Span {
                text: "\n".repeat(self.pending_sep),
                styles: self.separator_styles(),
                url: None,
                mermaid_svg: None,
            });
            self.pending_sep = 0;
        }
        self.first = false;
        let mut styles = self.separator_styles();
        styles.push(Style::Mermaid);
        self.spans.push(Span {
            text: MERMAID_PLACEHOLDER.to_owned(),
            styles,
            url: None,
            mermaid_svg: Some(svg),
        });
    }

    /// Styles active at the current parser position.
    #[must_use]
    pub fn context_styles(&self) -> Vec<Style> {
        let mut styles = self.inline.clone();
        if self.heading > 0 {
            styles.push(Style::Heading(self.heading));
        }
        if self.quote_depth > 0 {
            styles.push(Style::Quote(self.quote_depth));
        }
        styles
    }

    /// Emit the pending list prefix, if any.
    pub fn flush_prefix(&mut self) {
        if let Some(prefix) = self.item_prefix.take() {
            self.emit(&prefix, self.separator_styles());
        }
    }

    /// Consume a Markdown text event.
    pub fn text(&mut self, text: &str) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(text);
            return;
        }
        if let Some(buffer) = self.code_buf.as_mut() {
            buffer.push_str(text);
            return;
        }
        self.flush_prefix();
        self.emit(text, self.context_styles());
    }

    /// Consume an inline-code event.
    pub fn code(&mut self, code: &str) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(code);
            return;
        }
        self.flush_prefix();
        let mut styles = self.context_styles();
        styles.push(Style::Code);
        self.emit(code, styles);
    }

    /// Consume a soft or hard line break.
    pub fn line_break(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.cell.push(' ');
            return;
        }
        self.emit("\n", self.separator_styles());
    }

    /// Consume a task-list marker.
    pub fn task_marker(&mut self, checked: bool) {
        let marker = if checked { "[x] " } else { "[ ] " };
        if let Some(table) = self.table.as_mut() {
            table.cell.push_str(marker);
            return;
        }
        if self.item_prefix.is_some() {
            self.item_prefix = None;
            let prefix = format!("{}{marker}", self.item_indent);
            self.emit(&prefix, self.separator_styles());
        } else {
            self.emit(marker, Vec::new());
        }
    }

    /// Consume a pulldown-cmark start tag.
    pub fn start_tag(&mut self, tag: Tag<'_>) {
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
                        let lang = info.split_whitespace().next().unwrap_or("").to_owned();
                        (!lang.is_empty()).then_some(lang)
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
                let first_item = self.lists.last().is_none_or(|list| !list.emitted_any);
                if first_item && depth <= 1 {
                    self.block_start();
                } else {
                    self.sep(1);
                }
                let marker = match self.lists.last_mut() {
                    Some(list) => {
                        list.emitted_any = true;
                        match list.next {
                            Some(number) => {
                                list.next = Some(number + 1);
                                format!("{number}. ")
                            }
                            None => bullet_marker(depth),
                        }
                    }
                    None => bullet_marker(depth),
                };
                self.item_indent = " ".repeat(2 * depth.saturating_sub(1));
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

    /// Consume a pulldown-cmark end tag.
    pub fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => self.heading = 0,
            TagEnd::BlockQuote(_) => self.quote_depth = self.quote_depth.saturating_sub(1),
            TagEnd::CodeBlock => {
                if let Some(buffer) = self.code_buf.take() {
                    let code = buffer.trim_end_matches('\n').to_owned();
                    let language = self.code_lang.take();
                    if is_mermaid(language.as_deref())
                        && let Some(svg) = render_mermaid(&code, self.dark)
                    {
                        self.emit_mermaid(svg);
                    } else if !code.is_empty() || language.is_some() {
                        let mut styles = self.separator_styles();
                        styles.push(Style::CodeBlock);
                        if let Some(label) = language.as_deref() {
                            let mut language_styles = styles.clone();
                            language_styles.push(Style::Dim);
                            self.emit(&format!("{label}\n"), language_styles);
                        }
                        if !code.is_empty() {
                            let dark = self.dark;
                            emit_code_spans(self, &code, language.as_deref(), styles, dark);
                        }
                    }
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item => {
                if self.item_blocks.last() == Some(&0) {
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
                    table.rows.push(std::mem::take(&mut table.row));
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.row.push(std::mem::take(&mut table.cell));
                }
            }
            _ => {}
        }
    }

    /// Lay the buffered table out as aligned, padded rows.
    pub fn render_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        if table.rows.is_empty() {
            return;
        }

        let columns = table
            .alignments
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        let mut widths = vec![0usize; columns];
        for row in &table.rows {
            for (width, cell) in widths.iter_mut().zip(row.iter()) {
                *width = (*width).max(cell.width());
            }
        }

        let mut lines: Vec<(String, Vec<Style>)> = Vec::with_capacity(table.rows.len() + 1);
        for (row_index, row) in table.rows.iter().enumerate() {
            let cells: Vec<String> = widths
                .iter()
                .enumerate()
                .map(|(index, width)| {
                    pad_cell(
                        row.get(index).map_or("", String::as_str),
                        *width,
                        table
                            .alignments
                            .get(index)
                            .copied()
                            .unwrap_or(pulldown_cmark::Alignment::None),
                    )
                })
                .collect();
            if row_index == 0 {
                lines.push((cells.join(" │ "), vec![Style::Table, Style::Bold]));
                let rule = widths
                    .iter()
                    .map(|width| "─".repeat(*width))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                lines.push((rule, vec![Style::Table, Style::Dim]));
            } else {
                lines.push((cells.join(" │ "), vec![Style::Table]));
            }
        }
        let mut first = true;
        for (text, styles) in lines {
            if !first {
                self.sep(1);
            }
            first = false;
            self.emit(&text, styles);
        }
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderedMarkdown {
    /// Build a rendered document from its ordered text spans.
    #[must_use]
    pub fn new(spans: Vec<Span>) -> Self {
        Self { spans }
    }

    /// Borrow the ordered spans for a frontend renderer.
    #[must_use]
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }
}

/// Horizontal rule used by the plain Markdown renderer.
pub const RULE_LINE: &str = "────────────────────────────────────────";

/// Return the bullet marker for a nested list depth.
#[must_use]
pub fn bullet_marker(depth: usize) -> String {
    match depth {
        1 => "• ".to_owned(),
        2 => "◦ ".to_owned(),
        _ => "▪ ".to_owned(),
    }
}

/// Pad a table cell to a display width with the requested alignment.
#[must_use]
pub fn pad_cell(cell: &str, width: usize, align: pulldown_cmark::Alignment) -> String {
    use unicode_width::UnicodeWidthStr;

    let pad = width.saturating_sub(cell.width());
    match align {
        pulldown_cmark::Alignment::Right => format!("{}{}", " ".repeat(pad), cell),
        pulldown_cmark::Alignment::Center => {
            let left = pad / 2;
            format!("{}{}{}", " ".repeat(left), cell, " ".repeat(pad - left))
        }
        _ => format!("{}{}", cell, " ".repeat(pad)),
    }
}

/// Parse Markdown into frontend-neutral styled spans.
#[must_use]
pub fn render(source: &str) -> RenderedMarkdown {
    render_themed(source, false)
}

/// Parse Markdown, highlighting fenced code blocks for `dark` mode.
///
/// Light uses `InspiredGitHub`, dark uses `base16-ocean.dark`; only the
/// token foreground + emphasis is kept so the preview background from
/// `PreviewTags::apply_theme` shows through.
#[must_use]
pub fn render_themed(source: &str, dark: bool) -> RenderedMarkdown {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let mut renderer = Renderer::with_dark(dark);
    for event in Parser::new_ext(source, options) {
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
    RenderedMarkdown::new(renderer.spans)
}

/// Maximum highlighted code size: beyond this fall back to plain `CodeBlock`.
const MAX_HIGHLIGHT_BYTES: usize = 128 * 1024;
/// Maximum highlighted lines before falling back to plain `CodeBlock`.
const MAX_HIGHLIGHT_LINES: usize = 1000;
/// Placeholder text marking a rendered Mermaid diagram in the span stream.
/// A lone newline: the diagram widget is anchored at this span's position via
/// [`Span::mermaid_svg`], and the newline keeps block separation. It must not
/// contain `U+FFFC` or any other visible glyph — the anchor itself occupies a
/// buffer position, so any extra character renders as tofu after the picture.
pub const MERMAID_PLACEHOLDER: &str = "\n";
/// Maximum Mermaid source size: beyond this keep the plain code block so a
/// pasted dump cannot blow up SVG layout on every keystroke.
const MAX_MERMAID_BYTES: usize = 64 * 1024;

/// Whether a fenced code language selects the Mermaid renderer.
#[must_use]
pub fn is_mermaid(language: Option<&str>) -> bool {
    language.is_some_and(|lang| lang.trim().eq_ignore_ascii_case("mermaid"))
}

/// Render Mermaid `source` to SVG for `dark` mode.
///
/// Returns `None` for empty/oversized input or parse/render failure so the
/// caller can fall back to a plain code block. Pure and `Send`-safe, so the
/// editor's background render thread can call it.
#[must_use]
pub fn render_mermaid(source: &str, dark: bool) -> Option<Box<str>> {
    if source.trim().is_empty() || source.len() > MAX_MERMAID_BYTES {
        return None;
    }
    let mut theme = if dark {
        mermaid_svg::Theme::dark()
    } else {
        mermaid_svg::Theme::default_theme()
    }
    // CONTEXT: Adwaita's UI font keeps diagram labels visually consistent
    // with the surrounding GTK preview.
    .with_font("Cantarell, sans-serif");
    // CONTEXT: `gtk::Svg` cannot resolve the responsive `width="100%"` output
    // (no intrinsic height → zero-size picture in a TextView anchor), so emit
    // fixed pixel dimensions instead.
    theme.responsive = false;
    mermaid_svg::render_with(source, &theme)
        .ok()
        .filter(|svg| svg.starts_with("<svg"))
        .map(String::into_boxed_str)
}

/// Emit a fenced code body, syntax-highlighted when possible.
///
/// Falls back to a single plain `CodeBlock` span for unknown languages,
/// highlight errors, or oversized blocks.
fn emit_code_spans(
    renderer: &mut Renderer,
    code: &str,
    language: Option<&str>,
    base: Vec<Style>,
    dark: bool,
) {
    if code.len() > MAX_HIGHLIGHT_BYTES || code.lines().count() > MAX_HIGHLIGHT_LINES {
        renderer.emit(code, base);
        return;
    }
    let Some(tokens) = highlight_tokens(code, language, dark) else {
        renderer.emit(code, base);
        return;
    };
    for (text, syntax) in tokens {
        let mut styles = base.clone();
        styles.push(syntax);
        renderer.emit(&text, styles);
    }
}

/// Tokenize `code` into `(text, Style::Syntax)` runs.
///
/// Returns `None` when there is no known syntax or highlighting fails, so
/// the caller can fall back to plain rendering.
fn highlight_tokens(
    code: &str,
    language: Option<&str>,
    dark: bool,
) -> Option<Vec<(String, Style)>> {
    use std::sync::OnceLock;
    use syntect::easy::HighlightLines;
    use syntect::highlighting::{FontStyle, ThemeSet};
    use syntect::parsing::SyntaxSet;
    use syntect::util::LinesWithEndings;

    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let themes = THEMES.get_or_init(ThemeSet::load_defaults);

    let lang = language.unwrap_or("").trim();
    if lang.is_empty() {
        return None;
    }
    let syntax = resolve_syntax(syntaxes, lang)?;
    let theme_name = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let theme = themes
        .themes
        .get(theme_name)
        .or_else(|| themes.themes.values().next())?;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut out: Vec<(String, Style)> = Vec::new();
    for line in LinesWithEndings::from(code) {
        let ranges = highlighter.highlight_line(line, syntaxes).ok()?;
        for (style, text) in ranges {
            let syntax_style = Style::Syntax {
                fg: Rgb(style.foreground.r, style.foreground.g, style.foreground.b),
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
            };
            // Coalesce adjacent runs with the same style so a 300-line
            // block yields hundreds (not thousands) of spans. This keeps
            // the preview's per-span tag pass cheap.
            if let Some((last_text, last_style)) = out.last_mut()
                && *last_style == syntax_style
            {
                last_text.push_str(text);
            } else {
                out.push((text.to_owned(), syntax_style));
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Resolve a fence info string to a syntect syntax.
fn resolve_syntax<'a>(
    syntaxes: &'a syntect::parsing::SyntaxSet,
    lang: &str,
) -> Option<&'a syntect::parsing::SyntaxReference> {
    let key = lang.trim().to_ascii_lowercase();
    let normalized = match key.as_str() {
        "js" | "javascript" => "js",
        "ts" | "typescript" => "ts",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "sh" | "bash" | "zsh" | "shell" => "sh",
        "c++" | "cpp" | "cc" | "cxx" => "cpp",
        "c#" | "csharp" | "cs" => "cs",
        "py" | "python" => "py",
        "rs" | "rust" => "rs",
        "yml" | "yaml" => "yaml",
        "dockerfile" => "Dockerfile",
        "md" | "markdown" => "md",
        _ => key.as_str(),
    };
    syntaxes
        .find_syntax_by_extension(normalized)
        .or_else(|| syntaxes.find_syntax_by_token(normalized))
        .or_else(|| syntaxes.find_syntax_by_extension(&key))
        .or_else(|| syntaxes.find_syntax_by_token(&key))
        .or_else(|| syntaxes.find_syntax_by_token(lang.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bullet_marker_uses_nested_depth_symbols() {
        assert_eq!(bullet_marker(1), "• ");
        assert_eq!(bullet_marker(2), "◦ ");
        assert_eq!(bullet_marker(3), "▪ ");
    }

    #[test]
    fn pad_cell_aligns_text_to_display_width() {
        assert_eq!(pad_cell("a", 3, pulldown_cmark::Alignment::Right), "  a");
        assert_eq!(pad_cell("a", 3, pulldown_cmark::Alignment::Center), " a ");
        assert_eq!(pad_cell("a", 3, pulldown_cmark::Alignment::Left), "a  ");
    }

    #[test]
    fn rendered_markdown_exposes_spans_without_owning_a_frontend() {
        let rendered = RenderedMarkdown::new(vec![Span {
            text: "hello".into(),
            styles: vec![Style::Bold],
            url: None,
            mermaid_svg: None,
        }]);
        assert_eq!(rendered.spans()[0].text, "hello");
    }

    fn has_syntax(spans: &[Span]) -> bool {
        spans.iter().any(|s| {
            s.styles.contains(&Style::CodeBlock)
                && s.styles.iter().any(|st| matches!(st, Style::Syntax { .. }))
        })
    }

    #[test]
    fn rust_code_block_is_syntax_highlighted() {
        let spans = render("```rust\nfn main() {}\n```").spans().to_vec();
        assert!(has_syntax(&spans), "{spans:?}");
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert!(joined.contains("fn main()"), "{joined}");
    }

    #[test]
    fn unknown_language_falls_back_to_plain_code_block() {
        let spans = render("```nosuchlang123\nhello\n```").spans().to_vec();
        assert!(!has_syntax(&spans), "{spans:?}");
        assert!(spans.iter().any(|s| s.styles.contains(&Style::CodeBlock)));
    }

    #[test]
    fn language_alias_resolves() {
        let spans = render("```js\nconst x = 1;\n```").spans().to_vec();
        assert!(has_syntax(&spans), "{spans:?}");
    }

    #[test]
    fn oversized_code_block_skips_highlighting() {
        let big = "x".repeat(129 * 1024);
        let md = format!("```rust\n{big}\n```");
        let spans = render(&md).spans().to_vec();
        assert!(!has_syntax(&spans), "oversized block must fall back");
    }

    #[test]
    fn medium_300_line_block_is_highlighted() {
        let body = (0..300)
            .map(|i| format!("fn f{i}() {{}}"))
            .collect::<Vec<_>>()
            .join("\n");
        let md = format!("```rust\n{body}\n```");
        let spans = render(&md).spans().to_vec();
        assert!(has_syntax(&spans), "300-line block must highlight");
    }

    #[test]
    fn dark_theme_still_highlights() {
        let spans = render_themed("```python\nprint(1)\n```", true)
            .spans()
            .to_vec();
        assert!(has_syntax(&spans), "{spans:?}");
    }

    #[test]
    fn mermaid_flowchart_emits_diagram_span() {
        let spans = render("```mermaid\ngraph TD\nA --> B\n```")
            .spans()
            .to_vec();
        assert!(
            spans.iter().any(|s| s.styles.contains(&Style::Mermaid)),
            "{spans:?}"
        );
    }

    #[test]
    fn mermaid_dark_mode_renders_svg() {
        let spans = render_themed("```mermaid\ngraph TD\nA --> B\n```", true)
            .spans()
            .to_vec();
        assert!(
            spans.iter().any(|s| s.styles.contains(&Style::Mermaid)),
            "{spans:?}"
        );
    }

    #[test]
    fn mermaid_placeholder_carries_no_visible_label() {
        let spans = render("```mermaid\ngraph TD\nA --> B\n```")
            .spans()
            .to_vec();
        let placeholder = spans
            .iter()
            .find(|s| s.styles.contains(&Style::Mermaid))
            .map(|s| s.text.as_str())
            .unwrap_or("");
        assert!(!placeholder.contains("mermaid diagram"), "{placeholder:?}");
        assert!(!placeholder.contains('\u{FFFC}'), "{placeholder:?}");
    }

    #[test]
    fn mermaid_svg_emits_fixed_dimensions_for_gtk() {
        let svg = render_mermaid("graph TD\nA --> B", false).expect("svg");
        assert!(svg.contains("height="), "{svg:.120}");
    }

    #[test]
    fn multiple_mermaid_blocks_each_emit_a_diagram_span() {
        let md = "```mermaid\ngraph TD\nA --> B\n```\n\ntext\n\n```mermaid\npie\n\"A\" : 1\n```";
        let spans = render(md).spans().to_vec();
        let count = spans
            .iter()
            .filter(|s| s.styles.contains(&Style::Mermaid))
            .count();
        assert_eq!(count, 2, "{spans:?}");
    }

    #[test]
    fn mermaid_invalid_source_falls_back_to_code_block() {
        let spans = render("```mermaid\nnot a diagram {{{\n```")
            .spans()
            .to_vec();
        assert!(
            !spans.iter().any(|s| s.styles.contains(&Style::Mermaid)),
            "{spans:?}"
        );
    }

    #[test]
    fn mermaid_invalid_source_keeps_code_block_style() {
        let spans = render("```mermaid\nnot a diagram {{{\n```")
            .spans()
            .to_vec();
        assert!(
            spans.iter().any(|s| s.styles.contains(&Style::CodeBlock)),
            "{spans:?}"
        );
    }

    #[test]
    fn mermaid_language_match_is_case_insensitive() {
        assert!(is_mermaid(Some("Mermaid")));
    }

    #[test]
    fn mermaid_language_match_trims_whitespace() {
        assert!(is_mermaid(Some("  MERMAID  ")));
    }
}
