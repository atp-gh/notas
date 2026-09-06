//! Search boundary.

/// Escape free-form input into a safe FTS5 MATCH expression.
pub fn fts_query(user_input: &str) -> String {
    user_input
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}
