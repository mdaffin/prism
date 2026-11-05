//! The table's columns, how it is sorted, and the user's column regex.

use regex::Regex;

/// A column of the table that can be hidden: every one but the checkbox and the title columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Number,
    Checks,
    Review,
    Author,
    Updated,
}

impl Column {
    /// Every column, in the order the table shows them.
    pub const ALL: [Column; 5] = [
        Self::Number,
        Self::Checks,
        Self::Review,
        Self::Author,
        Self::Updated,
    ];

    /// The header, and the name in the columns popup.
    pub fn name(self) -> &'static str {
        match self {
            Self::Number => "#",
            Self::Checks => "Checks",
            Self::Review => "Review",
            Self::Author => "Author",
            Self::Updated => "Updated",
        }
    }

    /// What clicking the header sorts by.
    pub fn sort(self) -> Sort {
        match self {
            Self::Number => Sort::Number,
            Self::Checks => Sort::Checks,
            Self::Review => Sort::Review,
            Self::Author => Sort::Author,
            Self::Updated => Sort::Updated,
        }
    }
}

/// How the list is ordered. Whatever the sort, closed and merged PRs come last, as they can't be
/// acted on, and PRs that tie stay in the order GitHub listed them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// Passing first, then pending or without checks, then failing or in conflict.
    #[default]
    Checks,
    /// Lowest number, the oldest PR, first.
    Number,
    /// A to Z, ignoring case.
    Title,
    /// Those still needing a review, then those with changes requested, then approved ones.
    Review,
    /// A to Z by login, ignoring case.
    Author,
    /// The longest since anything happened to it first.
    Updated,
}

impl Sort {
    /// The next sort in the cycle, which comes back to the first.
    pub fn next(self) -> Self {
        match self {
            Self::Checks => Self::Number,
            Self::Number => Self::Title,
            Self::Title => Self::Review,
            Self::Review => Self::Author,
            Self::Author => Self::Updated,
            Self::Updated => Self::Checks,
        }
    }
}

/// Names the title columns can't use, as other parts of a PR already have them.
const RESERVED_COLUMNS: [&str; 13] = [
    "number", "title", "branch", "url", "changes", "body", "comments", "diff", "state", "checks",
    "runs", "review", "updated",
];

/// Compiles the column regex and checks its named groups can be columns.
pub fn parse_column_regex(pattern: &str) -> Result<Regex, String> {
    let regex = Regex::new(pattern).map_err(|error| error.to_string())?;
    let names = column_names(&regex);
    if names.is_empty() {
        return Err("the column regex needs at least one named group, e.g. '(?<package>\\S+) to (?<version>\\S+)'".to_string());
    }
    let clashes: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| RESERVED_COLUMNS.contains(name))
        .collect();
    if !clashes.is_empty() {
        return Err(format!(
            "the column regex's group names clash with built-in columns: {}",
            clashes.join(", ")
        ));
    }
    Ok(regex)
}

/// The names of the regex's groups, in order. They are the table's column headers.
pub fn column_names(regex: &Regex) -> Vec<String> {
    regex
        .capture_names()
        .flatten()
        .map(str::to_string)
        .collect()
}

/// What each named group matched in `title`, or empty text when the title doesn't match.
pub fn column_cells(regex: &Regex, title: &str) -> Vec<String> {
    let captures = regex.captures(title);
    column_names(regex)
        .iter()
        .map(|name| {
            captures
                .as_ref()
                .and_then(|captures| captures.name(name))
                .map_or("", |found| found.as_str())
                .to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUMP: &str = r"Bump (?<container>.+?)(?: from \S+)? to (?<version>\S+)$";

    #[test]
    fn the_columns_are_the_named_groups_in_order() {
        let regex = parse_column_regex(BUMP).unwrap();
        assert_eq!(column_names(&regex), ["container", "version"]);
        assert_eq!(
            column_cells(&regex, "Bump example-lib from 1.0.0 to 1.2.0"),
            ["example-lib", "1.2.0"]
        );
        assert_eq!(
            column_cells(&regex, "Bump example-lib to 1.2.0"),
            ["example-lib", "1.2.0"]
        );
        // A title that doesn't match, or a group that doesn't take part, gives an empty cell.
        assert_eq!(column_cells(&regex, "Fix a bug"), ["", ""]);
        let optional = parse_column_regex(r"(?<a>x)(?<b>y)?").unwrap();
        assert_eq!(column_cells(&optional, "x"), ["x", ""]);
    }

    #[test]
    fn a_bad_column_regex_is_rejected() {
        assert!(parse_column_regex("(unclosed").is_err());
        let error = parse_column_regex(r"(\S+) to (\S+)").unwrap_err();
        assert!(error.contains("at least one named group"));
        let error = parse_column_regex(r"(?<title>\S+) to (?<version>\S+)").unwrap_err();
        assert_eq!(
            error,
            "the column regex's group names clash with built-in columns: title"
        );
        let error = parse_column_regex(r"(?<checks>\S+) (?<review>\S+)").unwrap_err();
        assert!(error.ends_with("checks, review"));
    }
}
