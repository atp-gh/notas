//! Platform-neutral Markdown rendering data.

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
}

/// A text run carrying the styles active at that position.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    /// Display text for the run.
    pub text: String,
    /// Styles active for the run.
    pub styles: Vec<Style>,
    /// Destination URL when the span is link text.
    pub url: Option<String>,
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
}

impl Renderer {
    /// Create an empty renderer state.
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
    pub fn new(spans: Vec<Span>) -> Self {
        Self { spans }
    }

    /// Borrow the ordered spans for a frontend renderer.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }
}

/// Horizontal rule used by the plain Markdown renderer.
pub const RULE_LINE: &str = "────────────────────────────────────────";

/// Return the bullet marker for a nested list depth.
pub fn bullet_marker(depth: usize) -> String {
    match depth {
        1 => "• ".to_owned(),
        2 => "◦ ".to_owned(),
        _ => "▪ ".to_owned(),
    }
}

/// Pad a table cell to a display width with the requested alignment.
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
        }]);
        assert_eq!(rendered.spans()[0].text, "hello");
    }
}
