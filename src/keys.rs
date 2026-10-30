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
    key("Tab / Shift-Tab", "Move focus between the panes", None),
    key(
        "Up/Down, j/k",
        "Move the highlight, or scroll the pane",
        None,
    ),
    key("PgUp/PgDn", "The same, a page at a time", None),
    key("Space", "Toggle selection", None),
    key(
        "Enter",
        "Review the selected PRs, or the highlighted one",
        Some("Review"),
    ),
    key(
        "Ctrl-O",
        "Open the selected or highlighted PRs in the browser",
        None,
    ),
    key("Esc", "Clear the selection", None),
    key("q", "Quit, or close the current dialog", Some("Quit")),
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
