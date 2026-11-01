//! The keys of the list, in one table that the help popup and the status bar are both built
//! from, so they can't drift apart.

pub struct KeyHelp {
    pub keys: &'static str,
    pub action: &'static str,
    /// A short name for the status bar, if the key is shown there.
    pub bar: Option<&'static str>,
}

const fn key(keys: &'static str, action: &'static str, bar: Option<&'static str>) -> KeyHelp {
    KeyHelp { keys, action, bar }
}

pub const KEYS: &[KeyHelp] = &[
    key("?", "Show every key", Some("Help")),
    key("/", "Edit the GitHub search query", Some("Search")),
    key("f", "Filter the list as you type", None),
    key("Tab / Shift-Tab", "Move focus between the panes", None),
    key(
        "Up/Down, j/k",
        "Move the highlight, or scroll the pane",
        None,
    ),
    key(
        "PgUp/PgDn",
        "Scroll the pane a page, or move the table's highlight a page; the comments' highlight stays",
        None,
    ),
    key(
        "Home/End",
        "Jump to the first or last item, or the top or bottom of a pane",
        None,
    ),
    key(
        "Enter/Space (comments)",
        "Expand or collapse the highlighted comment; Up/Down scroll until the next one is on screen",
        None,
    ),
    key(
        "Left/Right, h/l",
        "Scroll the tables of the description or a comment sideways (also Shift-wheel)",
        None,
    ),
    key("Space", "Toggle selection", None),
    key(
        "Enter",
        "Review the selected PRs, or the highlighted one",
        Some("Review"),
    ),
    key(
        "o",
        "Open what the pane shows: a check's or comment's page, else the PR(s) like Ctrl-O",
        None,
    ),
    key(
        "Ctrl-O",
        "Open the selected or highlighted PRs in the browser",
        None,
    ),
    key(
        "Ctrl-A",
        "Select every visible PR with passing checks",
        None,
    ),
    key("Ctrl-R", "Run the query again, keeping the selection", None),
    key("Esc", "Clear the filter, then the selection", None),
    key(
        "q",
        "Quit, close the current dialog, or cancel a running batch",
        Some("Quit"),
    ),
    key("Ctrl-C", "Quit from anywhere", None),
];

/// The status bar's key hints, such as `?: Help | /: Search`.
pub fn bar() -> String {
    KEYS.iter()
        .filter_map(|help| help.bar.map(|bar| format!("{}: {bar}", help.keys)))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_is_built_from_the_table() {
        assert_eq!(bar(), "?: Help | /: Search | Enter: Review | q: Quit");
    }
}
