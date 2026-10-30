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

#[cfg(test)]
mod tests {
    use super::*;

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
