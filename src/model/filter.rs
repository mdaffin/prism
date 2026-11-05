//! The local filter box.

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// The local filter, matching like fzf: the characters of a term in order but not necessarily
/// together, with smart case (lowercase matches any case, an upper-case letter matches exactly).
/// Terms are separated by spaces and must all match, each against any column.
pub struct Filter {
    atoms: Vec<Atom>,
    matcher: Matcher,
    buffer: Vec<char>,
}

impl Filter {
    pub fn new(text: &str) -> Self {
        let atoms = text
            .split_whitespace()
            .map(|term| {
                Atom::new(
                    term,
                    CaseMatching::Smart,
                    Normalization::Smart,
                    AtomKind::Fuzzy,
                    false,
                )
            })
            .collect();
        Self {
            atoms,
            matcher: Matcher::new(Config::DEFAULT),
            buffer: Vec::new(),
        }
    }

    /// Whether a row with these columns passes. An empty filter passes every row.
    pub fn matches(&mut self, columns: &[String]) -> bool {
        self.atoms.iter().all(|atom| {
            columns.iter().any(|column| {
                let haystack = Utf32Str::new(column, &mut self.buffer);
                atom.score(haystack, &mut self.matcher).is_some()
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passes(filter: &str, columns: &[&str]) -> bool {
        let columns: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
        Filter::new(filter).matches(&columns)
    }

    #[test]
    fn a_filter_matches_characters_in_order_but_spread_out() {
        assert!(passes("rnv", &["app/renovate"]));
        assert!(!passes("vnr", &["app/renovate"]));
    }

    #[test]
    fn a_filter_with_no_match_passes_nothing() {
        assert!(!passes("xyz", &["#1", "app/renovate", "Bump alpha"]));
    }

    #[test]
    fn lowercase_matches_any_case_and_upper_case_matches_exactly() {
        assert!(passes("bump", &["Bump alpha"]));
        assert!(passes("Bump", &["Bump alpha"]));
        assert!(!passes("BUMP", &["Bump alpha"]));
        assert!(!passes("Bump", &["bump alpha"]));
    }

    #[test]
    fn every_term_must_match_some_column() {
        let columns = ["#123", "app/renovate", "Bump alpha"];
        assert!(passes("deps 123", &["#123", "deps"]));
        assert!(passes("renovate alpha", &columns));
        assert!(!passes("renovate beta", &columns));
    }

    #[test]
    fn an_empty_filter_passes_every_row() {
        assert!(passes("", &["#1"]));
        assert!(passes("   ", &[]));
    }

    #[test]
    fn a_term_does_not_match_across_columns() {
        assert!(!passes("1ren", &["#1", "app/renovate"]));
    }
}
