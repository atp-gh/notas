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
