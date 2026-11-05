//! Plain-text helpers: shortening, cleaning and picking lines out of text.

use std::sync::LazyLock;

use regex::Regex;

static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// Joins the lines of `text` with spaces, so it fits in one row.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The line of a regex error that says what is wrong: the one after `error:` if there is one, as
/// the first line of a syntax error only says that there is one.
pub fn regex_error_line(error: &str) -> &str {
    error
        .lines()
        .find_map(|line| line.trim().strip_prefix("error: "))
        .unwrap_or_else(|| first_line(error))
}

/// The first line of `text` that isn't blank, such as the headline of an error message.
pub fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// Removes HTML comments and runs of blank lines from a PR or comment body.
pub fn clean_body(body: Option<&str>) -> String {
    let body = body.unwrap_or_default().replace("\r\n", "\n");
    let body = HTML_COMMENT.replace_all(&body, "");
    let body = BLANK_LINES.replace_all(&body, "\n\n");
    body.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_line_skips_blank_ones() {
        assert_eq!(first_line("\n  headline  \nrest"), "headline");
        assert_eq!(first_line("one"), "one");
        assert_eq!(first_line(" \n"), "");
    }

    #[test]
    fn cleans_body() {
        assert_eq!(clean_body(None), "");
        assert_eq!(
            clean_body(Some(
                "<!-- a\nmulti-line -->Hello<!-- b -->\n\n\n\n\nWorld<!-- c -->\n"
            )),
            "Hello\n\nWorld"
        );
        assert_eq!(clean_body(Some("one\r\n\r\n\r\n\r\ntwo")), "one\n\ntwo");
        assert_eq!(clean_body(Some("keep\n\nthis")), "keep\n\nthis");
    }
}
