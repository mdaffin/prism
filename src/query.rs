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

/// The qualifiers GitHub's search takes, which are completed by name.
const QUALIFIERS: &[&str] = &[
    "is",
    "state",
    "author",
    "assignee",
    "label",
    "review",
    "status",
    "base",
    "head",
    "sort",
    "draft",
    "reviewed-by",
    "review-requested",
    "user-review-requested",
    "team-review-requested",
    "mentions",
    "commenter",
    "involves",
    "milestone",
    "project",
    "no",
    "language",
    "in",
    "archived",
    "created",
    "updated",
    "closed",
    "merged",
    "comments",
    "interactions",
    "reactions",
    "repo",
    "org",
    "user",
    "linked",
    "team",
];

const IS_VALUES: &[&str] = &[
    "pr", "issue", "open", "closed", "merged", "draft", "locked", "unlocked", "public", "private",
];
const STATE_VALUES: &[&str] = &["open", "closed"];
const REVIEW_VALUES: &[&str] = &["none", "required", "approved", "changes_requested"];
const STATUS_VALUES: &[&str] = &["pending", "success", "failure"];
const SORT_VALUES: &[&str] = &[
    "created-desc",
    "created-asc",
    "updated-desc",
    "updated-asc",
    "comments-desc",
    "comments-asc",
    "reactions-desc",
    "reactions-asc",
    "interactions-desc",
    "interactions-asc",
];

/// The most suggestions made.
const MAX_SUGGESTIONS: usize = 100;

/// What this repository has, for the qualifiers whose values are not fixed.
#[derive(Default)]
pub struct Candidates<'a> {
    pub authors: &'a [String],
    pub labels: &'a [String],
    pub branches: &'a [String],
}

/// What a suggestion replaces, and so what accepting it adds.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Kind {
    /// A qualifier's name, followed by a `:`.
    Name,
    /// A qualifier's value, followed by a space.
    Value,
}

/// The suggestions for the word at the cursor.
#[derive(Debug, PartialEq)]
pub struct Completion {
    kind: Kind,
    /// The characters of the query that accepting a suggestion replaces.
    range: std::ops::Range<usize>,
    /// The text to put there, best match first. A value with spaces in it has its quotes.
    pub suggestions: Vec<String>,
}

/// The words of `chars` as ranges. A quoted stretch is part of a word, as in `label:"a b"`.
fn words(chars: &[char]) -> Vec<std::ops::Range<usize>> {
    let mut words = Vec::new();
    let (mut start, mut quoted) = (None, false);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() && !quoted {
            if let Some(start) = start.take() {
                words.push(start..i);
            }
            continue;
        }
        quoted ^= c == '"';
        start.get_or_insert(i);
    }
    words.extend(start.map(|start| start..chars.len()));
    words
}

/// Whether `candidate` starts with `typed`, or else contains it, ignoring case: `Some(false)` for
/// the first, `Some(true)` for the second. Starts come first.
fn matches(candidate: &str, typed: &str) -> Option<bool> {
    let (candidate, typed) = (candidate.to_lowercase(), typed.to_lowercase());
    if candidate.starts_with(&typed) {
        Some(false)
    } else {
        candidate.contains(&typed).then_some(true)
    }
}

/// The candidates that fit `typed`, those that start with it first, each once.
fn filter<'a>(candidates: impl Iterator<Item = &'a str>, typed: &str) -> Vec<&'a str> {
    let mut found: Vec<(bool, &str)> = candidates
        .filter_map(|c| Some((matches(c, typed)?, c)))
        .collect();
    // A stable sort, so each group keeps the order the candidates came in.
    found.sort_by_key(|&(contains, _)| contains);
    let mut seen = std::collections::HashSet::new();
    found
        .into_iter()
        .map(|(_, c)| c)
        .filter(|c| seen.insert(*c))
        .collect()
}

/// The values to suggest for the qualifier `name`.
fn values<'a>(name: &str, have: &Candidates<'a>) -> Vec<&'a str> {
    let sorted = |names: &'a [String]| {
        let mut names: Vec<&str> = names.iter().map(String::as_str).collect();
        names.sort_by_key(|n| n.to_lowercase());
        names
    };
    match name.to_lowercase().as_str() {
        "is" => IS_VALUES.to_vec(),
        "state" => STATE_VALUES.to_vec(),
        "review" => REVIEW_VALUES.to_vec(),
        "status" => STATUS_VALUES.to_vec(),
        "sort" => SORT_VALUES.to_vec(),
        "author" | "assignee" => sorted(have.authors),
        "label" => sorted(have.labels),
        "base" | "head" => sorted(have.branches),
        _ => Vec::new(),
    }
}

/// What fits the word at `cursor` (a character index) in `query`, or `None` when nothing does.
///
/// A word that is being typed as a qualifier's name is completed to a name, after a `-` if there
/// is one, and a word after a `qualifier:` is completed to one of its values. Nothing is
/// suggested for an empty name, so that the list doesn't open at each space, or when the only
/// suggestion is what is already there.
pub fn complete(query: &str, cursor: usize, have: &Candidates) -> Option<Completion> {
    let chars: Vec<char> = query.chars().collect();
    let cursor = cursor.min(chars.len());
    let word = words(&chars)
        .into_iter()
        .find(|w| w.start <= cursor && cursor <= w.end)?;
    // A `-` negates the qualifier and isn't part of its name.
    let name_start = word.start + usize::from(chars[word.start] == '-');
    let colon = (name_start..word.end).find(|&i| chars[i] == ':');
    let text = |range: std::ops::Range<usize>| chars[range].iter().collect::<String>();

    let (kind, range, suggestions) = match colon {
        Some(colon) if cursor > colon => {
            let range = colon + 1..word.end;
            let typed = text(colon + 1..cursor);
            let typed = typed.strip_prefix('"').unwrap_or(&typed);
            let found = filter(values(&text(name_start..colon), have).into_iter(), typed);
            let quote = |v: &str| {
                if v.contains(char::is_whitespace) {
                    format!("\"{v}\"")
                } else {
                    v.to_string()
                }
            };
            (Kind::Value, range, found.into_iter().map(quote).collect())
        }
        _ => {
            let typed = text(name_start..cursor);
            if typed.is_empty() {
                return None;
            }
            let range = name_start..colon.unwrap_or(word.end);
            let found = filter(QUALIFIERS.iter().copied(), &typed);
            (
                Kind::Name,
                range,
                found.into_iter().map(String::from).collect(),
            )
        }
    };
    let mut suggestions: Vec<String> = suggestions;
    suggestions.truncate(MAX_SUGGESTIONS);
    if suggestions.is_empty() || suggestions == [text(range.clone())] {
        return None;
    }
    Some(Completion {
        kind,
        range,
        suggestions,
    })
}

impl Completion {
    /// The query with the suggestion at `index` put in, and where the cursor then is: after a `:`
    /// for a name, and after a space for a value. A `:` or whitespace that is already there is
    /// used rather than added again.
    pub fn accept(&self, query: &str, index: usize) -> (String, usize) {
        let chars: Vec<char> = query.chars().collect();
        let mut inserted: Vec<char> = self.suggestions[index].chars().collect();
        let mut end = self.range.end;
        let (suffix, there) = match self.kind {
            Kind::Name => (':', chars.get(end) == Some(&':')),
            Kind::Value => (' ', chars.get(end).is_some_and(|c| c.is_whitespace())),
        };
        if there {
            // Step over it.
            end += 1;
        }
        inserted.push(suffix);
        let cursor = self.range.start + inserted.len();
        let mut accepted: Vec<char> = chars[..self.range.start].to_vec();
        accepted.extend(inserted);
        accepted.extend(&chars[end.min(chars.len())..]);
        (accepted.into_iter().collect(), cursor)
    }
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

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// The suggestions for the query up to a `|`, which is the cursor, with the text after it.
    fn suggest(query: &str, have: &Candidates) -> Option<Vec<String>> {
        let cursor = query.chars().position(|c| c == '|').unwrap();
        let query = query.replace('|', "");
        complete(&query, cursor, have).map(|c| c.suggestions)
    }

    /// The query after accepting the first suggestion, with a `|` where the cursor is.
    fn accept(query: &str, have: &Candidates) -> String {
        let cursor = query.chars().position(|c| c == '|').unwrap();
        let query = query.replace('|', "");
        let (accepted, cursor) = complete(&query, cursor, have).unwrap().accept(&query, 0);
        let mut chars: Vec<char> = accepted.chars().collect();
        chars.insert(cursor, '|');
        chars.into_iter().collect()
    }

    #[test]
    fn completes_a_qualifier_name_from_what_is_typed() {
        let have = Candidates::default();
        assert_eq!(suggest("la|", &have), Some(strings(&["label", "language"])));
        assert_eq!(suggest("is:pr rev|", &have).unwrap()[0], "review");
        // Free text that fits no qualifier gets nothing, and neither does an empty word.
        assert_eq!(suggest("zzz|", &have), None);
        assert_eq!(suggest("is:pr |", &have), None);
        assert_eq!(suggest("|", &have), None);
    }

    #[test]
    fn a_name_that_is_already_complete_is_not_suggested_again() {
        assert_eq!(suggest("sort|", &Candidates::default()), None);
        assert_eq!(suggest("is:open|", &Candidates::default()), None);
    }

    #[test]
    fn completes_the_fixed_values() {
        let have = Candidates::default();
        assert_eq!(
            suggest("review:|", &have).unwrap().len(),
            REVIEW_VALUES.len()
        );
        assert_eq!(suggest("is:dr|", &have), Some(strings(&["draft"])));
        assert_eq!(suggest("state:Op|", &have), Some(strings(&["open"])));
        assert_eq!(suggest("status:|", &have).unwrap()[0], "pending");
    }

    #[test]
    fn completes_values_from_what_the_repository_has() {
        let (authors, labels, branches) = (
            strings(&["bob", "alice", "app/renovate"]),
            strings(&["deps", "Bug", "bug", "good first issue"]),
            strings(&["main", "feature/x", "renovate/y"]),
        );
        let have = Candidates {
            authors: &authors,
            labels: &labels,
            branches: &branches,
        };
        assert_eq!(
            suggest("author:a|", &have),
            Some(strings(&["alice", "app/renovate"]))
        );
        // Sorted ignoring case, each once, with those starting with the word first.
        assert_eq!(suggest("label:b|", &have), Some(strings(&["Bug", "bug"])));
        assert_eq!(
            suggest("base:|", &have).unwrap(),
            ["feature/x", "main", "renovate/y"]
        );
        assert_eq!(suggest("head:ren|", &have), Some(strings(&["renovate/y"])));
        // One that only contains the word comes after those that start with it.
        assert_eq!(
            suggest("author:r|", &have),
            Some(strings(&["app/renovate"]))
        );
        // Nothing is known about this one.
        assert_eq!(suggest("milestone:|", &have), None);
    }

    #[test]
    fn a_negated_qualifier_is_completed_like_the_plain_one() {
        let labels = strings(&["deps"]);
        let have = Candidates {
            labels: &labels,
            ..Candidates::default()
        };
        assert_eq!(suggest("-label:d|", &have), Some(strings(&["deps"])));
        assert_eq!(suggest("-lab|", &have).unwrap()[0], "label");
        assert_eq!(accept("-lab|", &have), "-label:|");
        assert_eq!(suggest("-|", &have), None);
    }

    #[test]
    fn a_value_with_spaces_is_quoted_and_a_quoted_word_is_completed() {
        let labels = strings(&["good first issue", "bug"]);
        let have = Candidates {
            labels: &labels,
            ..Candidates::default()
        };
        assert_eq!(
            suggest("label:g|", &have),
            Some(strings(&["\"good first issue\"", "bug"]))
        );
        assert_eq!(
            suggest("label:\"good f|", &have),
            Some(strings(&["\"good first issue\""]))
        );
        assert_eq!(
            accept("label:\"good f|\" x", &have),
            "label:\"good first issue\" |x"
        );
    }

    #[test]
    fn completes_the_word_at_the_cursor_in_the_middle_of_the_query() {
        let labels = strings(&["deps"]);
        let have = Candidates {
            labels: &labels,
            ..Candidates::default()
        };
        assert_eq!(
            accept("is:pr lab| author:x", &have),
            "is:pr label:| author:x"
        );
        assert_eq!(
            accept("is:pr label:d| author:x", &have),
            "is:pr label:deps |author:x"
        );
        // The name is replaced, and the value after it is kept.
        assert_eq!(accept("la|:deps", &have), "label:|deps");
        // Whichever word the cursor is in, not the whole query.
        assert_eq!(
            suggest("author:x la| state:open", &have).unwrap()[0],
            "label"
        );
    }

    #[test]
    fn accepting_adds_a_colon_or_a_space_at_the_end() {
        let labels = strings(&["deps"]);
        let have = Candidates {
            labels: &labels,
            ..Candidates::default()
        };
        assert_eq!(accept("is:pr lab|", &have), "is:pr label:|");
        assert_eq!(accept("label:de|", &have), "label:deps |");
        // Whitespace that is already there is not doubled.
        assert_eq!(accept("label:de| x", &have), "label:deps |x");
    }

    #[test]
    fn the_cursor_may_be_past_the_end_or_the_query_empty() {
        assert_eq!(complete("", 0, &Candidates::default()), None);
        assert_eq!(
            complete("is:", 99, &Candidates::default())
                .unwrap()
                .suggestions[0],
            "pr"
        );
    }
}
