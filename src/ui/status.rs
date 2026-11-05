use super::*;

/// The hints of the modes other than the list, for the end of the status line. The list's are
/// built by `fit_status_line`, which has to fit them to the width.
pub(super) fn mode_hints(app: &App) -> Option<&'static str> {
    match &app.mode {
        Mode::Confirm(_) | Mode::ConfirmOpen(_) => Some(keys::CHOOSE_HINTS),
        Mode::Help(_) => Some(keys::HELP_HINTS),
        Mode::Failure(_) => Some(keys::FAILURE_HINTS),
        Mode::PickColumns(_) => Some(keys::PICK_COLUMNS_HINTS),
        Mode::Running(_) => Some(keys::RUNNING_HINTS),
        Mode::Results(_) => Some(keys::RESULTS_HINTS),
        Mode::Loading | Mode::List => match app.typing {
            Typing::Search => Some(match app.completion() {
                Some((_, Some(_))) => keys::CHOSEN_HINTS,
                Some((_, None)) => keys::SUGGESTIONS_HINTS,
                None => keys::SEARCH_HINTS,
            }),
            Typing::Filter => Some(keys::FILTER_HINTS),
            Typing::Columns => Some(keys::COLUMNS_HINTS),
            Typing::Nothing => None,
        },
    }
}

/// The status bar: what is going on, then the keys that do something now, then the page `o`
/// opens, in `width` columns.
pub(super) fn status_line(app: &App, width: u16) -> String {
    let mut parts = Vec::new();
    if app.list.searching {
        parts.push(format!("{} Searching", spinner(app)));
    }
    if app.refreshing() {
        parts.push(format!("{} Refreshing", spinner(app)));
    }
    if !app.selected.is_empty() {
        let hidden = app.hidden_selected();
        let mut selected = format!("{} selected", app.selected.len());
        if hidden > 0 {
            selected.push_str(&format!(" ({hidden} hidden)"));
        }
        parts.push(selected);
    }
    if let Some(failure) = &app.status.failure {
        parts.push(format!("Refresh failed: {failure}"));
    }
    if app.status.paused() {
        parts.push("Refresh paused".to_string());
    }
    parts.extend(app.note.clone());
    if let Some(hints) = mode_hints(app) {
        parts.push(hints.to_string());
        return parts.join(" | ");
    }
    let mut hints = keys::bar_hints(app.focus);
    let page = match (app.focus, app.highlighted_page()) {
        (_, Some(Page::Url(url))) => Some(url),
        // With no page there is no `o` hint either: it would promise one.
        (focus, Some(Page::Missing)) => {
            hints.retain(|hint| !hint.for_page);
            Some(if focus == Pane::Comments {
                "No page, o opens the PR".to_string()
            } else {
                "No page".to_string()
            })
        }
        (_, None) => None,
    };
    fit_status_line(parts, page, hints, usize::from(width))
}

/// The shortest the URL is cut to before the hints that aren't essential go.
pub(super) const MIN_URL_WIDTH: usize = 24;

/// Joins the status `parts`, the `hints` and the `page` (a URL, or a note that there is none) in
/// `width` columns, the page last so that it takes the space the keys leave. A URL too long for it
/// is cut in the middle first; if it is still too long, the
/// hints that aren't essential go from the end, and the URL is cut as short as it must be.
pub(super) fn fit_status_line(
    mut parts: Vec<String>,
    page: Option<String>,
    mut hints: Vec<keys::Hint>,
    width: usize,
) -> String {
    let join = |parts: &[String], page: &Option<String>, hints: &[keys::Hint]| {
        let mut all: Vec<&str> = parts.iter().map(String::as_str).collect();
        all.extend(hints.iter().map(|hint| hint.text.as_str()));
        all.extend(page.as_deref());
        all.join(" | ")
    };
    let Some(page) = page else {
        parts.extend(hints.into_iter().map(|hint| hint.text));
        return parts.join(" | ");
    };
    let width_of = |text: &str| unicode_width::UnicodeWidthStr::width(text);
    // What the line takes up apart from the page, with its separator.
    let others = |hints: &[keys::Hint]| width_of(&join(&parts, &None, hints)) + 3;
    let room = |hints: &[keys::Hint]| width.saturating_sub(others(hints));
    while room(&hints) < MIN_URL_WIDTH.min(width_of(&page)) {
        let Some(last) = hints.iter().rposition(|hint| !hint.essential) else {
            break;
        };
        hints.remove(last);
    }
    let page = middle_cut(&page, room(&hints));
    join(&parts, &Some(page), &hints)
}

/// `text` cut in the middle with `…` to fit `width` columns.
pub(super) fn middle_cut(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let kept = width.saturating_sub(1);
    let head = kept.div_ceil(2);
    let tail = kept - head;
    let head: String = chars[..head].iter().collect();
    let tail: String = chars[chars.len() - tail..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::*;

    #[test]
    fn the_status_bar_shows_the_focused_panes_keys() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        for expected in [
            "Enter: Review | o: Open | y: Copy | ?: Help",
            "Enter, o: Open | y: Copy | ?: Help",
            "Enter, Space: Expand | o: Open | y: Copy | ?: Help",
            "?: Help | /: Search",
            "?: Help | /: Search",
            "?: Help | /: Search",
        ] {
            let screen = lines(&draw(&mut app, 200, 30));
            let bar = screen.last().unwrap();
            assert!(bar.contains(expected), "{:?} in {bar:?}", app.focus);
            // The keys of the other panes are not on it.
            assert_eq!(
                bar.matches("o: Open").count(),
                expected.matches("o: Open").count()
            );
            key(&mut app, KeyCode::Tab);
        }
    }

    /// A PR whose failing check has a page and whose passing one has none.
    fn app_with_check_pages() -> App {
        app_with(vec![pr_with(
            1,
            "Bump alpha",
            "OPEN",
            "",
            "MERGEABLE",
            serde_json::json!([
                {"__typename": "CheckRun", "name": "b", "workflowName": "CI",
                 "status": "COMPLETED", "conclusion": "FAILURE",
                 "detailsUrl": "https://example.test/job"},
                {"__typename": "CheckRun", "name": "c", "workflowName": "CI",
                 "status": "COMPLETED", "conclusion": "SUCCESS"},
            ]),
        )])
    }

    fn tab(app: &mut App, times: usize) {
        for _ in 0..times {
            key(app, KeyCode::Tab);
        }
    }

    #[test]
    fn the_status_bar_shows_the_highlighted_checks_url_and_says_when_it_has_none() {
        let mut app = app_with_check_pages();
        // The table has no page to show.
        assert!(!status_bar(&mut app, 160).contains("https://"));

        tab(&mut app, 1);
        let bar = status_bar(&mut app, 160);
        assert!(
            bar.contains("Enter, o: Open | y: Copy | ?: Help | /: Search | q: Quit | https://example.test/job"),
            "{bar}"
        );

        // The passing check has no page, so there is no `o` hint either.
        press(&mut app, "j");
        let bar = status_bar(&mut app, 160);
        assert!(bar.ends_with("q: Quit | No page"), "{bar}");
        assert!(
            !bar.contains("o: Open") && !bar.contains("y: Copy"),
            "{bar}"
        );
    }

    #[test]
    fn the_status_bar_shows_the_highlighted_comments_url() {
        let mut app = app_with_check_pages();
        let details = serde_json::from_value(serde_json::json!({
            "headRefName": "branch",
            "body": "",
            "comments": [
                {"createdAt": "2026-01-02T03:00:00Z", "body": "a",
                 "url": "https://example.test/pull/1#issuecomment-1"},
                {"createdAt": "2026-01-02T03:00:00Z", "body": "b"},
            ],
            "additions": 1,
            "deletions": 1,
        }))
        .unwrap();
        app.details_loaded(1, Ok(details));
        tab(&mut app, 2);
        let bar = status_bar(&mut app, 160);
        assert!(
            bar.contains("Enter, Space: Expand | o: Open | y: Copy | ?: Help | /: Search | q: Quit | https://example.test/pull/1#issuecomment-1"),
            "{bar}"
        );
        // `o` still opens the PR for a comment with no page of its own.
        app.drawn.comment_layout = CommentLayout {
            items: vec![(0, 1), (1, 1)],
            viewport: 5,
        };
        press(&mut app, "j");
        let bar = status_bar(&mut app, 160);
        assert!(
            bar.contains(
                "Enter, Space: Expand | ?: Help | /: Search | q: Quit | No page, o opens the PR"
            ),
            "{bar}"
        );
        assert!(!bar.contains("o: Open"), "{bar}");
    }

    #[test]
    fn the_status_bar_cuts_a_long_url_then_drops_hints_but_keeps_o() {
        let long = "https://github.com/example/prism/actions/runs/123456789/job/987654321";
        let mut app = app_with(vec![pr_with(
            1,
            "Bump alpha",
            "OPEN",
            "",
            "MERGEABLE",
            serde_json::json!([
                {"__typename": "CheckRun", "name": "b", "workflowName": "CI",
                 "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": long},
            ]),
        )]);
        tab(&mut app, 1);
        // Wide enough for all of it.
        assert!(status_bar(&mut app, 200).contains(long));
        // Room for the hints, so only the URL is cut, in the middle.
        let bar = status_bar(&mut app, 110);
        assert!(
            bar.contains("https://github.com/") && bar.contains("…"),
            "{bar}"
        );
        assert!(
            bar.contains("o: Open | y: Copy | ?: Help | /: Search | q: Quit | https://"),
            "{bar}"
        );
        // Narrower still: the other hints go, `o` stays.
        let bar = status_bar(&mut app, 50);
        assert!(bar.contains("…") && bar.contains("Enter, o: Open"), "{bar}");
        assert!(!bar.contains("?: Help"), "{bar}");
        assert!(bar.chars().count() <= 50, "{bar}");
    }

    #[test]
    fn the_status_bar_says_what_was_copied_until_the_next_key() {
        let mut app = app_with_check_pages();
        tab(&mut app, 1);
        press(&mut app, "y");
        let bar = status_bar(&mut app, 160);
        assert!(bar.starts_with("Copied the URL | Enter, o: Open"), "{bar}");
        press(&mut app, "k");
        assert!(!status_bar(&mut app, 160).contains("Copied"));
    }

    #[test]
    fn the_status_bar_says_when_the_refresh_is_paused() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        assert!(!status_bar(&mut app, 160).contains("Refresh paused"));
        press(&mut app, "p");
        assert!(status_bar(&mut app, 160).starts_with("Refresh paused | "));
        press(&mut app, "p");
        app.update(Msg::FocusLost);
        assert!(status_bar(&mut app, 160).contains("Refresh paused"));
        app.update(Msg::FocusGained);
        assert!(!status_bar(&mut app, 160).contains("Refresh paused"));
    }

    #[test]
    fn cuts_text_in_the_middle() {
        assert_eq!(middle_cut("abcdefghij", 5), "ab…ij");
        assert_eq!(middle_cut("abcdefghij", 4), "ab…j");
        assert_eq!(middle_cut("abc", 5), "abc");
        assert_eq!(middle_cut("abc", 3), "abc");
    }
}
