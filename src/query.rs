/// Builds the starting search query from the arguments: `is:pr state:open [author:X] [search]`.
pub fn initial(author: Option<&str>, search: &str) -> String {
    let mut parts = vec!["is:pr".to_string(), "state:open".to_string()];
    if let Some(author) = author.map(str::trim).filter(|a| !a.is_empty()) {
        parts.push(format!("author:{author}"));
    }
    let search = search.trim();
    if !search.is_empty() {
        parts.push(search.to_string());
    }
    parts.join(" ")
}

/// Whether every PR the query finds has the same author, so a column of them says nothing: there is
/// one `author:X` and no `OR` or parentheses that could widen it. Anything harder to read than
/// that counts as no.
pub fn names_one_author(query: &str) -> bool {
    let mut tokens = query.split_whitespace();
    let simple = tokens.clone().all(|t| t != "OR" && !t.contains(['(', ')']));
    let authors = tokens
        .by_ref()
        .filter(|t| t.strip_prefix("author:").is_some_and(|v| !v.is_empty()))
        .count();
    simple && authors == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_author_qualifier_names_one_author() {
        assert!(names_one_author(
            "is:pr state:open author:app/renovate label:deps"
        ));
        assert!(names_one_author("author:alice"));
    }

    #[test]
    fn no_author_or_a_negated_one_names_none() {
        assert!(!names_one_author("is:pr state:open"));
        assert!(!names_one_author("is:pr -author:app/renovate"));
        assert!(!names_one_author("author:"));
    }

    #[test]
    fn free_text_does_not_count() {
        assert!(!names_one_author("is:pr alice"));
        assert!(!names_one_author("is:pr renovate author"));
    }

    #[test]
    fn several_authors_or_a_wider_query_names_no_one() {
        assert!(!names_one_author("author:alice author:bob"));
        assert!(!names_one_author("author:alice OR label:deps"));
        assert!(!names_one_author("(author:alice label:a) OR label:b"));
    }

    #[test]
    fn no_author_or_search() {
        assert_eq!(initial(None, ""), "is:pr state:open");
    }

    #[test]
    fn author_only() {
        assert_eq!(
            initial(Some("app/renovate"), ""),
            "is:pr state:open author:app/renovate"
        );
    }

    #[test]
    fn search_only() {
        assert_eq!(
            initial(None, "label:deps -label:major"),
            "is:pr state:open label:deps -label:major"
        );
    }

    #[test]
    fn author_and_search() {
        assert_eq!(
            initial(Some("app/renovate"), "label:deps"),
            "is:pr state:open author:app/renovate label:deps"
        );
    }

    #[test]
    fn blank_values_are_skipped() {
        assert_eq!(initial(Some("  "), "   "), "is:pr state:open");
    }

    #[test]
    fn values_are_trimmed() {
        assert_eq!(
            initial(Some(" app/renovate "), "  label:deps  "),
            "is:pr state:open author:app/renovate label:deps"
        );
    }
}
