//! The keys of the list, in one table that the help popup and the status bar are both built
//! from, so they can't drift apart. Each key is for every pane or for one, and the status bar
//! shows the focused pane's keys. The hints for the other modes, which the status bar shows in
//! place of the list's, are here too.

use crate::app::Pane;

/// Choosing in the confirm dialogs.
pub const CHOOSE_HINTS: &str = "←/→ Tab: Choose | Enter: Confirm | Esc: Cancel";
pub const HELP_HINTS: &str = "Esc ? q: Close";
pub const RUNNING_HINTS: &str = "q: Cancel | Ctrl-C: Quit";
pub const RESULTS_HINTS: &str = "Enter/Esc: Close";
/// Editing the search box.
pub const SEARCH_HINTS: &str = "Enter: Search | Esc: Cancel";
/// Editing the filter box.
pub const FILTER_HINTS: &str = "Enter: Keep filter | Esc: Clear filter";

pub struct KeyHelp {
    pub keys: &'static str,
    pub action: &'static str,
    /// A short name for the status bar, if the key is shown there.
    pub bar: Option<&'static str>,
    /// The pane the key is for, or `None` for a key that works in every pane.
    pub pane: Option<Pane>,
}

/// A key that works in every pane.
const fn key(keys: &'static str, action: &'static str, bar: Option<&'static str>) -> KeyHelp {
    KeyHelp {
        keys,
        action,
        bar,
        pane: None,
    }
}

/// A key that is for one pane. Where a key means something else in another pane, that pane
/// has a row of its own.
const fn pane_key(
    pane: Pane,
    keys: &'static str,
    action: &'static str,
    bar: Option<&'static str>,
) -> KeyHelp {
    KeyHelp {
        keys,
        action,
        bar,
        pane: Some(pane),
    }
}

/// The groups of the `?` popup, in order: the keys of every pane, then those of each pane that
/// has any, under the pane's title.
pub const GROUPS: [(Option<Pane>, &str); 5] = [
    (None, "Everywhere"),
    (Some(Pane::Table), "Pull requests"),
    (Some(Pane::Checks), "Checks"),
    (Some(Pane::Comments), "Comments"),
    (Some(Pane::Description), "Description"),
];

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
    key("Space", "Toggle selection (not in the comments)", None),
    key("Ctrl-O", "Open the selected PRs in the browser", None),
    key("Ctrl-G", "Review the selected PRs", None),
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
    pane_key(
        Pane::Table,
        "Enter",
        "Review the highlighted PR",
        Some("Review"),
    ),
    pane_key(
        Pane::Table,
        "o",
        "Open the highlighted PR in the browser",
        Some("Open"),
    ),
    pane_key(
        Pane::Checks,
        "Enter, o",
        "Open the highlighted check's page",
        Some("Open"),
    ),
    pane_key(
        Pane::Comments,
        "Up/Down, j/k",
        "Go comment by comment, scrolling a line at a time until the next one is on screen",
        None,
    ),
    pane_key(
        Pane::Comments,
        "Enter, Space",
        "Expand or collapse the highlighted comment",
        Some("Expand"),
    ),
    pane_key(
        Pane::Comments,
        "o",
        "Open the highlighted comment, or its PR if it has no page",
        Some("Open"),
    ),
    pane_key(
        Pane::Comments,
        "Left/Right, h/l",
        "Scroll the open comments' tables sideways (also Shift-wheel)",
        None,
    ),
    pane_key(
        Pane::Description,
        "Left/Right, h/l",
        "Scroll the description's tables sideways (also Shift-wheel)",
        None,
    ),
];

/// The keys of one group of the `?` popup: those of a pane, or of every pane for `None`.
pub fn group(pane: Option<Pane>) -> impl Iterator<Item = &'static KeyHelp> {
    KEYS.iter().filter(move |help| help.pane == pane)
}

/// The status bar's key hints when `focus` has the focus, such as `Enter: Review | ?: Help`: the
/// pane's own keys first, then those of every pane.
pub fn bar(focus: Pane) -> String {
    [Some(focus), None]
        .into_iter()
        .flat_map(group)
        .filter_map(|help| help.bar.map(|bar| format!("{}: {bar}", help.keys)))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_has_the_focused_panes_keys_first() {
        let tail = "?: Help | /: Search | q: Quit";
        for (pane, own) in [
            (Pane::Table, "Enter: Review | o: Open"),
            (Pane::Checks, "Enter, o: Open"),
            (Pane::Comments, "Enter, Space: Expand | o: Open"),
        ] {
            assert_eq!(bar(pane), format!("{own} | {tail}"));
        }
        // These panes have no keys of their own on the bar.
        for pane in [Pane::Description, Pane::Diff] {
            assert_eq!(bar(pane), tail);
        }
    }

    #[test]
    fn every_pane_with_keys_has_a_group_in_the_popup() {
        for help in KEYS {
            assert!(
                GROUPS.iter().any(|(pane, _)| *pane == help.pane),
                "{} has no group",
                help.keys
            );
        }
    }
}
