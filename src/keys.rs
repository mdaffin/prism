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
pub const SEARCH_HINTS: &str = "Enter: Search | Up/Down: History | Esc: Cancel";
/// Editing the search box with suggestions listed and none chosen.
pub const SUGGESTIONS_HINTS: &str = "Enter: Search | Tab/Up/Down: Choose | Esc: Close";
/// Editing the search box with a suggestion chosen.
pub const CHOSEN_HINTS: &str = "Enter: Take | Tab/Down: Next | Shift-Tab/Up: Previous | Esc: Close";
/// Editing the filter box.
pub const FILTER_HINTS: &str = "Enter: Keep filter | Esc: Clear filter";
/// Editing the columns box.
pub const COLUMNS_HINTS: &str = "Enter: Apply | Esc: Cancel";

/// The status bar's name for the key that opens the highlighted item, which the bar keeps when
/// it has to drop hints.
pub const OPEN: &str = "Open";
/// The status bar's name for the key that copies the highlighted item's URL.
pub const COPY: &str = "Copy";

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
pub const GROUPS: [(Option<Pane>, &str); 6] = [
    (None, "Everywhere"),
    (Some(Pane::Table), "Pull requests"),
    (Some(Pane::Checks), "Checks"),
    (Some(Pane::Comments), "Comments"),
    (Some(Pane::Description), "Description"),
    (Some(Pane::Diff), "Diff"),
];

pub const KEYS: &[KeyHelp] = &[
    key("?", "Show every key", Some("Help")),
    key("/", "Edit the GitHub search query", Some("Search")),
    key("f", "Filter the list as you type", None),
    key(
        "s",
        "Sort the list by the next column (checks, number, title, review, then when it was updated); the header shows which, and ▲ or ▼ which way",
        None,
    ),
    key("S", "Reverse the sort", None),
    key(
        "c",
        "Edit the regex whose named groups become the table's columns",
        None,
    ),
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
    key(
        "p",
        "Pause or resume the timed refresh, which also pauses while the terminal is out of focus",
        None,
    ),
    key("Esc", "Clear the filter, then the selection", None),
    key(
        "q",
        "Quit, close the current dialog, or cancel a running batch",
        Some("Quit"),
    ),
    key("Ctrl-C", "Quit from anywhere", None),
    key(
        "Search box: Tab, Down",
        "Choose the next suggestion, the first to begin with; Enter takes the chosen one",
        None,
    ),
    key(
        "Search box: Shift-Tab, Up",
        "Choose the previous suggestion; without a list, Up goes through the queries searched before",
        None,
    ),
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
        Some(OPEN),
    ),
    pane_key(
        Pane::Table,
        "y",
        "Copy the highlighted PR's URL",
        Some(COPY),
    ),
    pane_key(
        Pane::Checks,
        "Enter, o",
        "Open the highlighted check's page",
        Some(OPEN),
    ),
    pane_key(
        Pane::Checks,
        "y",
        "Copy the highlighted check's URL",
        Some(COPY),
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
        Some(OPEN),
    ),
    pane_key(
        Pane::Comments,
        "y",
        "Copy the highlighted comment's URL, or its PR's if it has no page",
        Some(COPY),
    ),
    pane_key(
        Pane::Comments,
        "Left/Right, h/l",
        "Scroll the open comments' tables sideways (also Shift-wheel)",
        None,
    ),
    pane_key(
        Pane::Table,
        "Left/Right, h/l",
        "Scroll the list sideways, keeping the number in view (also Shift-wheel)",
        None,
    ),
    pane_key(
        Pane::Checks,
        "Left/Right, h/l",
        "Scroll the check names sideways (also Shift-wheel)",
        None,
    ),
    pane_key(
        Pane::Diff,
        "Left/Right, h/l",
        "Scroll the diff sideways (also Shift-wheel)",
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

/// A hint of the status bar, such as `Enter: Review`.
pub struct Hint {
    pub text: String,
    /// Whether the bar keeps it when it is too narrow for all of them.
    pub essential: bool,
    /// Whether it is about the highlighted item's page, so the bar leaves it out when there is
    /// none.
    pub for_page: bool,
}

/// The status bar's key hints when `focus` has the focus: the pane's own keys first, then those
/// of every pane.
pub fn bar_hints(focus: Pane) -> Vec<Hint> {
    [Some(focus), None]
        .into_iter()
        .flat_map(group)
        .filter_map(|help| {
            help.bar.map(|bar| Hint {
                text: format!("{}: {bar}", help.keys),
                essential: bar == OPEN,
                for_page: bar == OPEN || bar == COPY,
            })
        })
        .collect()
}

/// The hints joined, such as `Enter: Review | ?: Help`.
#[cfg(test)]
pub fn bar(focus: Pane) -> String {
    let hints: Vec<String> = bar_hints(focus).into_iter().map(|hint| hint.text).collect();
    hints.join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_has_the_focused_panes_keys_first() {
        let tail = "?: Help | /: Search | q: Quit";
        for (pane, own) in [
            (Pane::Table, "Enter: Review | o: Open | y: Copy"),
            (Pane::Checks, "Enter, o: Open | y: Copy"),
            (Pane::Comments, "Enter, Space: Expand | o: Open | y: Copy"),
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
