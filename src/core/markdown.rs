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
}
