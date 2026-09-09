//! FTS5 MATCH query building and full-text search.
//!
//! The query builder is a pure function: user input in, safe MATCH
//! expression out. The database query lives in
//! [`crate::core::repository::Repository::search`], which keeps this module
//! free of I/O and easy to test exhaustively.

/// Escape free-form user input into a safe FTS5 MATCH expression: each
/// whitespace-separated token becomes a quoted phrase, and embedded double
/// quotes are doubled so they cannot break out of the phrase.
///
/// An input with no tokens (empty or all whitespace) yields an empty
/// string, which callers must treat as "no search".
///
/// # Examples
///
/// ```
/// use notas::core::search::fts_query;
///
/// assert_eq!(fts_query("hello world"), "\"hello\" \"world\"");
/// // Quotes are doubled, not used to inject operators.
/// assert_eq!(fts_query("say \"hi\""), "\"say\" \"\"\"hi\"\"\"");
/// assert_eq!(fts_query("   "), "");
/// ```
#[must_use]
pub fn fts_query(user_input: &str) -> String {
    user_input
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_whitespace_queries_yield_empty_expression() {
        assert_eq!(fts_query(""), "");
        assert_eq!(fts_query("   "), "");
        assert_eq!(fts_query("\t\n"), "");
    }

    #[test]
    fn multiple_words_become_quoted_phrases() {
        assert_eq!(fts_query("budget review"), "\"budget\" \"review\"");
    }

    #[test]
    fn embedded_quotes_are_doubled_not_operators() {
        assert_eq!(fts_query(r#"say "hi""#), "\"say\" \"\"\"hi\"\"\"");
    }

    #[test]
    fn unicode_tokens_are_preserved() {
        assert_eq!(fts_query("中文 テスト"), "\"中文\" \"テスト\"");
    }

    #[test]
    fn fts_operators_are_neutralized_by_quoting() {
        // Without quoting these would be NEAR/OR/NOT operators or column
        // filters; as quoted phrases they are plain terms.
        assert_eq!(fts_query("NOT OR NEAR"), "\"NOT\" \"OR\" \"NEAR\"");
        assert_eq!(fts_query("title:abc"), "\"title:abc\"");
    }
}
