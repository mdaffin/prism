use super::work::StatusFetch;
use super::*;
use crate::gh::{PrStatus, ReviewDecision};
use crate::testing::{app_with, ctrl_g, pr_with, press};
use crossterm::event::{KeyCode, KeyModifiers};

fn pr(number: u64, state: &str) -> Pr {
    pr_with(number, "t", state, "", "MERGEABLE", serde_json::json!([]))
}

fn prs(numbers: impl IntoIterator<Item = u64>) -> Vec<Pr> {
    numbers
        .into_iter()
        .map(|number| pr(number, "OPEN"))
        .collect()
}

fn app_with_rows(count: u64) -> App {
    app_with(prs(1..=count))
}

/// The PR numbers of the fetches an app started.
fn started(fetches: Vec<(u64, FetchId)>) -> Vec<u64> {
    fetches.into_iter().map(|(number, _)| number).collect()
}

fn numbers(app: &App) -> Vec<u64> {
    app.rows.iter().map(|row| row.pr.number).collect()
}

#[test]
fn fetches_the_initial_query_once() {
    let mut app = App::new(String::new(), "is:pr state:open".to_string(), false);
    assert_eq!(
        app.list_to_fetch(),
        Some((1, "is:pr state:open".to_string()))
    );
    assert_eq!(app.list_to_fetch(), None);
}

#[test]
fn esc_in_the_list_clears_the_selection_and_never_quits() {
    let mut app = app_with_rows(3);
    press(&mut app, " j ");
    assert_eq!(app.selected.len(), 2);
    key(&mut app, KeyCode::Esc);
    assert!(app.selected.is_empty());
    key(&mut app, KeyCode::Esc);
    assert!(!app.should_quit);
    press(&mut app, "q");
    assert!(app.should_quit);
}

#[test]
fn help_opens_with_question_mark_and_closes_again() {
    let mut app = app_with_rows(3);
    for close in [KeyCode::Esc, KeyCode::Char('?'), KeyCode::Char('q')] {
        press(&mut app, "?");
        assert!(matches!(app.mode, Mode::Help(_)));
        key(&mut app, close);
        assert!(matches!(app.mode, Mode::List));
    }
    // `q` closed the popup rather than quitting.
    assert!(!app.should_quit);
}

#[test]
fn search_box_takes_every_key_while_editing() {
    let mut app = app_with_rows(3);
    press(&mut app, "/");
    assert!(app.typing == Typing::Search);
    press(&mut app, " q/jk");
    assert!(!app.should_quit);
    assert_eq!(app.query.value(), "is:pr q/jk");
    assert_eq!(app.table.selected(), Some(0));
}

#[test]
fn esc_restores_the_submitted_query() {
    let mut app = app_with_rows(3);
    press(&mut app, "/ x");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(app.typing != Typing::Search);
    assert_eq!(app.query.value(), "is:pr");
    assert_eq!(app.list_to_fetch(), None);
}

#[test]
fn up_and_down_in_the_search_box_walk_the_submitted_queries() {
    let mut app = app_with_rows(3);
    for query in ["x", "y"] {
        press(&mut app, &format!("/{query}"));
        key(&mut app, KeyCode::Enter);
    }
    press(&mut app, "/ half");
    key(&mut app, KeyCode::Up);
    assert_eq!(app.query.value(), "is:prxy");
    key(&mut app, KeyCode::Up);
    assert_eq!(app.query.value(), "is:prx");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    // What was half typed comes back.
    assert_eq!(app.query.value(), "is:prxy half");
    assert!(app.typing == Typing::Search);
}

#[test]
fn enter_fetches_the_new_query_and_resets_the_highlight() {
    let mut app = app_with_rows(3);
    app.table.select(Some(2));
    press(&mut app, "/ is:closed");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(app.typing != Typing::Search);
    let (id, query) = app.list_to_fetch().unwrap();
    assert_eq!(query, "is:pr is:closed");
    assert!(app.list.searching);

    app.update(Msg::PrsLoaded(id, Ok(prs([7, 8]))));
    assert!(!app.list.searching);
    assert_eq!(numbers(&app), [7, 8]);
    assert_eq!(app.table.selected(), Some(0));
}

#[test]
fn results_from_an_older_fetch_are_dropped() {
    let mut app = app_with_rows(3);
    press(&mut app, "/");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
    let (old, _) = app.list_to_fetch().unwrap();
    press(&mut app, "/");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
    let (new, _) = app.list_to_fetch().unwrap();

    app.update(Msg::PrsLoaded(new, Ok(prs([5]))));
    app.update(Msg::PrsLoaded(old, Ok(prs([6]))));
    assert_eq!(numbers(&app), [5]);
}

fn titled(number: u64, title: &str) -> Pr {
    let mut pr = pr(number, "OPEN");
    pr.title = title.to_string();
    pr
}

/// Three open PRs titled "Bump alpha", "Bump beta" and "Update gamma".
fn app_with_titles() -> App {
    app_with(vec![
        titled(1, "Bump alpha"),
        titled(2, "Bump beta"),
        titled(3, "Update gamma"),
    ])
}

fn visible_numbers(app: &App) -> Vec<u64> {
    app.visible_rows().map(|row| row.pr.number).collect()
}

#[test]
fn the_filter_matches_each_visible_column_in_any_case() {
    let mut app = app_with_titles();
    press(&mut app, "fbump");
    assert_eq!(visible_numbers(&app), [1, 2]);
    for (text, expected) in [
        ("#3", vec![3]),
        ("no checks", vec![1, 2, 3]),
        ("needed", vec![1, 2, 3]),
        ("approved", vec![]),
    ] {
        key(&mut app, KeyCode::Esc);
        press(&mut app, "f");
        press(&mut app, text);
        assert_eq!(visible_numbers(&app), expected, "{text}");
    }
}

#[test]
fn the_filter_is_fuzzy_with_smart_case_and_every_term_must_match() {
    let mut app = app_with_titles();
    for (text, expected) in [
        ("bmp", vec![1, 2]),
        ("bmpl", vec![1]),
        ("Bump", vec![1, 2]),
        ("BUMP", vec![]),
        ("bump alpha", vec![1]),
        ("alpha 1", vec![1]),
        ("zzz", vec![]),
        ("", vec![1, 2, 3]),
    ] {
        key(&mut app, KeyCode::Esc);
        press(&mut app, "f");
        press(&mut app, text);
        assert_eq!(visible_numbers(&app), expected, "{text}");
    }
}

#[test]
fn the_filter_is_fuzzy_across_the_author() {
    let mut prs = vec![pr(1, "OPEN"), pr(2, "OPEN")];
    prs[1].author.login = "app/renovate".to_string();
    let mut app = app_with(prs);
    press(&mut app, "frnv");
    assert_eq!(visible_numbers(&app), [2]);
}

#[test]
fn the_filter_matches_an_author() {
    let mut prs = vec![pr(1, "OPEN"), pr(2, "OPEN"), pr(3, "OPEN")];
    prs[1].author.login = "app/renovate".to_string();
    let mut app = app_with(prs);
    press(&mut app, "frenovate");
    assert_eq!(visible_numbers(&app), [2]);
}

#[test]
fn the_filter_matches_closed_and_merged_as_drawn() {
    let mut app = App::new(String::new(), "is:pr".to_string(), false);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(
        id,
        Ok(vec![pr(1, "OPEN"), pr(2, "MERGED"), pr(3, "CLOSED")]),
    ));
    press(&mut app, "fmerged");
    assert_eq!(visible_numbers(&app), [2]);
}

#[test]
fn the_highlight_stays_on_its_pr_while_it_is_visible() {
    let mut app = app_with_titles();
    press(&mut app, "jf");
    assert_eq!(app.highlighted().unwrap().pr.number, 2);
    // Row 2 is now the first visible row, but still highlighted.
    press(&mut app, "beta");
    assert_eq!(visible_numbers(&app), [2]);
    assert_eq!(app.highlighted().unwrap().pr.number, 2);
    // Once it is hidden, the first visible row is highlighted.
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Backspace);
    press(&mut app, "gamma");
    assert_eq!(app.highlighted().unwrap().pr.number, 3);
}

#[test]
fn moving_stays_within_the_visible_rows() {
    let mut app = app_with_titles();
    press(&mut app, "fbump");
    key(&mut app, KeyCode::Enter);
    press(&mut app, "jjj");
    assert_eq!(app.highlighted().unwrap().pr.number, 2);
    press(&mut app, "kkk");
    assert_eq!(app.highlighted().unwrap().pr.number, 1);
}

#[test]
fn enter_keeps_the_filter_and_esc_clears_it_before_the_selection() {
    let mut app = app_with_titles();
    press(&mut app, " fgamma");
    key(&mut app, KeyCode::Enter);
    assert!(app.typing != Typing::Filter);
    assert_eq!(visible_numbers(&app), [3]);
    // Esc in the list clears the filter first and leaves the selection.
    key(&mut app, KeyCode::Esc);
    assert_eq!(visible_numbers(&app), [1, 2, 3]);
    assert_eq!(app.selected, HashSet::from([1]));
    key(&mut app, KeyCode::Esc);
    assert!(app.selected.is_empty());
}

#[test]
fn esc_in_the_box_clears_the_filter() {
    let mut app = app_with_titles();
    press(&mut app, "fgamma");
    key(&mut app, KeyCode::Esc);
    assert!(app.typing != Typing::Filter);
    assert_eq!(app.filter.value(), "");
    assert_eq!(visible_numbers(&app), [1, 2, 3]);
}

#[test]
fn the_filter_box_takes_every_key_while_editing() {
    let mut app = app_with_titles();
    press(&mut app, "fq j/");
    assert!(!app.should_quit);
    assert!(app.typing != Typing::Search);
    assert_eq!(app.filter.value(), "q j/");
}

#[test]
fn hidden_selected_prs_stay_selected_and_are_reviewed() {
    let mut app = app_with_titles();
    press(&mut app, " jj ");
    assert_eq!(app.selected, HashSet::from([1, 3]));
    press(&mut app, "fbeta");
    key(&mut app, KeyCode::Enter);
    assert_eq!(visible_numbers(&app), [2]);
    assert_eq!(app.selected, HashSet::from([1, 3]));
    assert_eq!(app.hidden_selected(), 2);

    ctrl_g(&mut app);
    assert_eq!(confirm_numbers(&app), [1, 3]);
}

#[test]
fn a_new_search_clears_the_filter() {
    let mut app = app_with_titles();
    press(&mut app, "fgamma");
    key(&mut app, KeyCode::Enter);
    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(vec![titled(7, "Bump alpha")])));
    assert_eq!(app.filter.value(), "");
    assert_eq!(visible_numbers(&app), [7]);
}

#[test]
fn a_refreshed_row_stays_where_it_was_even_if_it_no_longer_matches() {
    let mut app = app_with_titles();
    press(&mut app, "falpha");
    assert_eq!(visible_numbers(&app), [1]);
    app.update(Msg::PrRefreshed(1, Ok(titled(1, "Something else"))));
    assert_eq!(visible_numbers(&app), [1]);
    assert_eq!(app.highlighted().unwrap().pr.title, "Something else");
}

#[test]
fn rows_the_filter_hides_are_still_prefetched() {
    let mut app = app_with_titles();
    press(&mut app, "fgamma");
    assert_eq!(started(app.details_to_fetch()), [3, 1, 2]);
}

fn ctrl_a(app: &mut App) {
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::CONTROL,
    )));
}

#[test]
fn ctrl_a_selects_open_prs_with_passing_checks_only() {
    let mut closed = pr_with_checks(4, "passing");
    closed.state = PrState::Merged;
    let prs = vec![
        pr_with_checks(1, "passing"),
        pr_with_checks(2, "pending"),
        pr_with_checks(3, "none"),
        closed,
        pr_with_checks(5, "passing"),
    ];
    let mut app = app_with(prs);
    ctrl_a(&mut app);
    assert_eq!(app.selected, HashSet::from([1, 5]));
    // Nothing starts: it only selects.
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn ctrl_a_adds_to_the_selection_and_only_from_visible_rows() {
    let mut app = App::new(String::new(), "is:pr".to_string(), false);
    let (id, _) = app.list_to_fetch().unwrap();
    let mut first = pr_with_checks(1, "passing");
    first.title = "keep".to_string();
    let mut second = pr_with_checks(2, "passing");
    second.title = "other".to_string();
    let mut pending = pr_with_checks(3, "pending");
    pending.title = "keep".to_string();
    app.update(Msg::PrsLoaded(id, Ok(vec![first, second, pending])));
    // Row 3 is pending, so Ctrl-A won't pick it, but it is already selected.
    press(&mut app, "jj ");
    press(&mut app, "fkeep");
    key(&mut app, KeyCode::Enter);
    ctrl_a(&mut app);
    assert_eq!(app.selected, HashSet::from([1, 3]));
}

fn ctrl_o(app: &mut App) {
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
}

const PANES: [Pane; 5] = [
    Pane::Table,
    Pane::Checks,
    Pane::Comments,
    Pane::Description,
    Pane::Diff,
];

#[test]
fn ctrl_o_and_ctrl_g_do_nothing_when_none_are_selected() {
    for pane in PANES {
        let mut app = app_with_rows(3);
        app.focus = pane;
        press(&mut app, "j");
        ctrl_o(&mut app);
        ctrl_g(&mut app);
        assert!(app.opens_to_run().is_empty(), "{pane:?}");
        assert!(matches!(app.mode, Mode::List), "{pane:?}");
    }
}

#[test]
fn ctrl_o_opens_the_selection_from_every_pane() {
    for pane in PANES {
        let mut app = app_with_rows(3);
        press(&mut app, " jj ");
        app.focus = pane;
        ctrl_o(&mut app);
        assert_eq!(app.opens_to_run(), [1, 3], "{pane:?}");
    }
}

#[test]
fn ctrl_g_reviews_the_selection_from_every_pane() {
    for pane in PANES {
        let mut app = app_with_rows(3);
        press(&mut app, " jj ");
        app.focus = pane;
        ctrl_g(&mut app);
        let Mode::Confirm(confirm) = &app.mode else {
            panic!("no review dialog from {pane:?}");
        };
        let reviewed: Vec<u64> = confirm.prs.iter().map(|pr| pr.number).collect();
        assert_eq!(reviewed, [1, 3], "{pane:?}");
    }
}

#[test]
fn ctrl_a_and_ctrl_r_work_from_every_pane() {
    for pane in PANES {
        let mut app = app_with(vec![
            pr_with_checks(1, "passing"),
            pr_with_checks(2, "pending"),
        ]);
        app.focus = pane;
        ctrl_a(&mut app);
        assert_eq!(app.selected, HashSet::from([1]), "{pane:?}");
        ctrl_r(&mut app);
        assert!(app.list_to_fetch().is_some(), "{pane:?}");
    }
}

#[test]
fn ctrl_o_opens_the_selected_prs_in_table_order() {
    let mut app = app_with_rows(4);
    // Selected out of order: 1, then 3, then 2.
    press(&mut app, " jj k ");
    ctrl_o(&mut app);
    assert_eq!(app.opens_to_run(), [1, 2, 3]);
}

#[test]
fn ctrl_o_asks_before_opening_more_than_five() {
    let mut app = app_with_rows(6);
    for _ in 0..6 {
        press(&mut app, " j");
    }
    ctrl_o(&mut app);
    assert!(app.opens_to_run().is_empty());
    assert!(matches!(app.mode, Mode::ConfirmOpen(_)));
    // Cancel leaves everything alone.
    press(&mut app, "l");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert!(app.opens_to_run().is_empty());

    ctrl_o(&mut app);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.opens_to_run(), [1, 2, 3, 4, 5, 6]);
}

#[test]
fn five_selected_prs_open_without_asking() {
    let mut app = app_with_rows(5);
    for _ in 0..5 {
        press(&mut app, " j");
    }
    ctrl_o(&mut app);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.opens_to_run().len(), 5);
}

#[test]
fn a_pr_that_fails_to_open_is_shown_with_the_error() {
    let mut app = app_with_rows(2);
    app.update(Msg::Opened(2, Err(color_eyre::eyre::eyre!("no browser"))));
    assert_eq!(
        app.errors.text().as_deref(),
        Some("Opening #2 failed: no browser")
    );
    app.update(Msg::Opened(1, Ok(())));
    assert_eq!(app.errors.text().unwrap().lines().count(), 1);
}

/// An app whose PR 1 has a failing check with a page, then two passing ones: one with no page
/// and a status context with one. The checks pane lists them in that order.
fn app_with_check_pages() -> App {
    let pr: Pr = serde_json::from_value(serde_json::json!({
        "number": 1,
        "title": "t",
        "state": "OPEN",
        "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "b", "workflowName": "CI",
             "status": "COMPLETED", "conclusion": "FAILURE",
             "detailsUrl": "https://example.test/job"},
            {"__typename": "CheckRun", "name": "c", "workflowName": "CI",
             "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "StatusContext", "context": "ci/x", "state": "SUCCESS",
             "targetUrl": "https://example.test/build"},
        ],
        "mergeable": "MERGEABLE",
        "reviewDecision": "",
        "url": "https://example.test/pull/1",
        "author": {"login": "alice"},
        "updatedAt": "2026-01-01T00:00:00Z",
    }))
    .unwrap();
    let mut app = app_with(vec![pr]);
    app.drawn.checks_layout = ChecksLayout {
        count: 3,
        first: 0,
        viewport: 5,
    };
    app
}

#[test]
fn o_opens_the_highlighted_checks_page_or_says_it_has_none() {
    let mut app = app_with_check_pages();
    focus(&mut app, Pane::Checks);
    press(&mut app, "o");
    assert_eq!(app.pages_to_run(), ["https://example.test/job"]);
    assert!(app.opens_to_run().is_empty());

    // The second check has no page: nothing opens and the error area says so.
    press(&mut app, "jo");
    assert!(app.pages_to_run().is_empty());
    assert_eq!(
        app.errors.text().as_deref(),
        Some("The highlighted check has no page")
    );
    // The message goes away with the next open.
    press(&mut app, "jo");
    assert_eq!(app.pages_to_run(), ["https://example.test/build"]);
    assert_eq!(app.errors.text(), None);
}

#[test]
fn o_opens_the_highlighted_comment_and_falls_back_to_the_pr() {
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
    app.drawn.comment_layout = CommentLayout {
        items: vec![(0, 1), (1, 1)],
        viewport: 5,
    };
    focus(&mut app, Pane::Comments);
    press(&mut app, "o");
    assert_eq!(
        app.pages_to_run(),
        ["https://example.test/pull/1#issuecomment-1"]
    );
    // The second comment has no URL, so the PR opens.
    press(&mut app, "jo");
    assert!(app.pages_to_run().is_empty());
    assert_eq!(app.opens_to_run(), [1]);
}

#[test]
fn o_opens_a_reviews_page_and_a_threads_first_comment() {
    let mut app = app_with_check_pages();
    let details = serde_json::from_value(serde_json::json!({
        "headRefName": "branch",
        "body": "",
        "comments": [
            {"createdAt": "2026-01-02T03:00:00Z", "body": "approved",
             "url": "https://example.test/pull/1#pullrequestreview-1",
             "kind": {"Review": "APPROVED"}},
            {"createdAt": "2026-01-02T04:00:00Z", "body": "first",
             "url": "https://example.test/pull/1#discussion_r1",
             "kind": {"Thread": {"path": "a.rs", "line": 3, "resolved": false, "outdated": false}},
             "replies": [{"createdAt": "2026-01-02T05:00:00Z", "body": "reply"}]},
        ],
        "additions": 1,
        "deletions": 1,
    }))
    .unwrap();
    app.details_loaded(1, Ok(details));
    app.drawn.comment_layout = CommentLayout {
        items: vec![(0, 1), (1, 1)],
        viewport: 5,
    };
    focus(&mut app, Pane::Comments);
    press(&mut app, "o");
    assert_eq!(
        app.pages_to_run(),
        ["https://example.test/pull/1#pullrequestreview-1"]
    );
    press(&mut app, "jo");
    assert_eq!(
        app.pages_to_run(),
        ["https://example.test/pull/1#discussion_r1"]
    );
}

#[test]
fn o_in_the_table_opens_only_the_highlighted_pr() {
    let mut app = app_with_rows(3);
    press(&mut app, " j");
    press(&mut app, "o");
    assert_eq!(app.opens_to_run(), [2]);
    assert!(app.pages_to_run().is_empty());
}

#[test]
fn o_does_nothing_in_the_description_and_the_diff() {
    let mut app = app_with_rows(2);
    press(&mut app, " j");
    for pane in [Pane::Description, Pane::Diff] {
        focus(&mut app, pane);
        press(&mut app, "o");
        assert!(app.opens_to_run().is_empty(), "{pane:?}");
        assert!(app.pages_to_run().is_empty(), "{pane:?}");
    }
}

#[test]
fn enter_in_the_checks_opens_the_highlighted_check_like_o() {
    let mut app = app_with_check_pages();
    focus(&mut app, Pane::Checks);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.pages_to_run(), ["https://example.test/job"]);
    assert!(matches!(app.mode, Mode::List));

    // The second check has no page.
    press(&mut app, "j");
    key(&mut app, KeyCode::Enter);
    assert!(app.pages_to_run().is_empty());
    assert_eq!(
        app.errors.text().as_deref(),
        Some("The highlighted check has no page")
    );
}

#[test]
fn enter_does_nothing_in_the_description_and_the_diff() {
    let mut app = app_with_rows(2);
    press(&mut app, " ");
    for pane in [Pane::Description, Pane::Diff] {
        focus(&mut app, pane);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::List), "{pane:?}");
        assert!(app.opens_to_run().is_empty(), "{pane:?}");
        assert!(app.pages_to_run().is_empty(), "{pane:?}");
    }
}

#[test]
fn enter_in_the_comments_expands_the_highlighted_comment() {
    let mut app = app_with_rows(1);
    app.drawn.comment_layout = CommentLayout {
        items: vec![(0, 1), (1, 1)],
        viewport: 5,
    };
    focus(&mut app, Pane::Comments);
    key(&mut app, KeyCode::Enter);
    assert!(app.scroll.comments_open.contains(&0));
    assert!(matches!(app.mode, Mode::List));
    key(&mut app, KeyCode::Enter);
    assert!(!app.scroll.comments_open.contains(&0));
}

#[test]
fn y_in_the_table_copies_the_highlighted_prs_url() {
    let mut app = app_with_rows(3);
    press(&mut app, " j");
    press(&mut app, "y");
    assert_eq!(
        app.copy_to_run().as_deref(),
        Some("https://example.test/pull/2")
    );
    assert_eq!(app.note.as_deref(), Some("Copied the PR's URL"));
    // It is taken once, and the next key clears the note.
    assert_eq!(app.copy_to_run(), None);
    press(&mut app, "j");
    assert_eq!(app.note, None);
    assert_eq!(app.copy_to_run(), None);
}

#[test]
fn y_in_the_checks_copies_the_highlighted_checks_url_or_says_it_has_none() {
    let mut app = app_with_check_pages();
    focus(&mut app, Pane::Checks);
    press(&mut app, "y");
    assert_eq!(
        app.copy_to_run().as_deref(),
        Some("https://example.test/job")
    );
    assert_eq!(app.note.as_deref(), Some("Copied the URL"));

    press(&mut app, "jy");
    assert_eq!(app.copy_to_run(), None);
    assert_eq!(
        app.note.as_deref(),
        Some("The highlighted check has no page to copy")
    );
}

#[test]
fn y_in_the_comments_copies_the_comments_url_and_falls_back_to_the_pr() {
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
    app.drawn.comment_layout = CommentLayout {
        items: vec![(0, 1), (1, 1)],
        viewport: 5,
    };
    focus(&mut app, Pane::Comments);
    press(&mut app, "y");
    assert_eq!(
        app.copy_to_run().as_deref(),
        Some("https://example.test/pull/1#issuecomment-1")
    );
    press(&mut app, "jy");
    assert_eq!(
        app.copy_to_run().as_deref(),
        Some("https://example.test/pull/1")
    );
    assert_eq!(app.note.as_deref(), Some("Copied the PR's URL"));
}

#[test]
fn y_copies_nothing_in_the_description_and_the_diff() {
    let mut app = app_with_rows(2);
    for pane in [Pane::Description, Pane::Diff] {
        focus(&mut app, pane);
        press(&mut app, "y");
        assert_eq!(app.copy_to_run(), None, "{pane:?}");
        assert_eq!(app.note, None, "{pane:?}");
    }
}

#[test]
fn a_page_that_fails_to_open_is_shown_with_the_error() {
    let mut app = app_with_rows(1);
    app.update(Msg::PageOpened(Err(color_eyre::eyre::eyre!("no browser"))));
    assert_eq!(
        app.errors.text().as_deref(),
        Some("Opening the page failed: no browser")
    );
}

#[test]
fn space_toggles_the_highlighted_pr_until_new_results() {
    let mut app = app_with_rows(3);
    press(&mut app, " jj ");
    assert_eq!(app.selected, HashSet::from([1, 3]));
    press(&mut app, "k ");
    assert_eq!(app.selected, HashSet::from([1, 2, 3]));
    press(&mut app, " ");
    assert_eq!(app.selected, HashSet::from([1, 3]));

    // Not cleared until the new results arrive.
    press(&mut app, "/");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
    let (id, _) = app.list_to_fetch().unwrap();
    assert_eq!(app.selected.len(), 2);
    app.update(Msg::PrsLoaded(id, Ok(prs([1, 2, 3]))));
    assert!(app.selected.is_empty());
}

fn key(app: &mut App, code: KeyCode) {
    app.update(Msg::Key(KeyEvent::from(code)));
}

fn confirm(app: &App) -> &Confirm {
    match &app.mode {
        Mode::Confirm(confirm) => confirm,
        _ => panic!("the confirm dialog isn't open"),
    }
}

fn confirm_numbers(app: &App) -> Vec<u64> {
    confirm(app).prs.iter().map(|pr| pr.number).collect()
}

#[test]
fn enter_confirms_the_highlighted_pr_when_none_are_selected() {
    let mut app = app_with_rows(3);
    press(&mut app, "j");
    key(&mut app, KeyCode::Enter);
    assert_eq!(confirm_numbers(&app), [2]);
    assert_eq!(confirm(&app).choices().len(), 3);
}

#[test]
fn enter_confirms_only_the_highlighted_pr_even_with_a_selection() {
    let mut app = app_with_rows(3);
    press(&mut app, " jj ");
    key(&mut app, KeyCode::Enter);
    assert_eq!(confirm_numbers(&app), [3]);
}

#[test]
fn closed_and_merged_prs_sort_after_open_ones() {
    let prs = vec![
        pr(1, "MERGED"),
        pr(2, "OPEN"),
        pr(3, "CLOSED"),
        pr(4, "OPEN"),
    ];
    let app = app_with(prs);
    assert_eq!(numbers(&app), [2, 4, 1, 3]);
}

#[test]
fn closed_and_merged_prs_are_left_out() {
    let prs = vec![pr(1, "OPEN"), pr(2, "CLOSED"), pr(3, "MERGED")];
    let mut app = app_with(prs);
    press(&mut app, " j j ");
    ctrl_g(&mut app);
    assert_eq!(confirm_numbers(&app), [1]);
    assert_eq!(confirm(&app).left_out, [2, 3]);

    // With only closed PRs there is nothing to choose but Cancel.
    key(&mut app, KeyCode::Esc);
    press(&mut app, "k");
    app.selected.clear();
    key(&mut app, KeyCode::Enter);
    assert!(confirm(&app).prs.is_empty());
    assert_eq!(confirm(&app).choices(), [Choice::Cancel]);
}

#[test]
fn the_dialog_moves_between_choices_and_closes() {
    let mut app = app_with_rows(3);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Right);
    assert_eq!(confirm(&app).choice, 1);
    for _ in 0..5 {
        key(&mut app, KeyCode::Tab);
    }
    assert_eq!(confirm(&app).choice, 2);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));

    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, Mode::List));
    assert!(!app.should_quit);
}

#[test]
fn a_list_loading_under_the_dialog_keeps_it_open() {
    let mut app = app_with_rows(3);
    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    key(&mut app, KeyCode::Enter);
    app.update(Msg::PrsLoaded(id, Ok(prs([7]))));
    assert_eq!(confirm_numbers(&app), [1]);
}

/// Selects the first `count` rows and chooses Approve.
fn start_approving(app: &mut App, count: usize) {
    for _ in 0..count {
        press(app, " j");
    }
    ctrl_g(app);
    key(app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Running(_)));
}

fn approve(app: &mut App, number: u64, result: Result<()>) {
    let epoch = app.batch_epoch();
    app.update(Msg::Approved(epoch, number, result));
}

#[test]
fn approvals_run_at_most_four_at_once() {
    let mut app = app_with_rows(6);
    start_approving(&mut app, 6);
    assert_eq!(app.approvals_to_run(), [1, 2, 3, 4]);
    assert!(app.approvals_to_run().is_empty());
    approve(&mut app, 2, Ok(()));
    assert_eq!(app.approvals_to_run(), [5]);
    approve(&mut app, 1, Ok(()));
    approve(&mut app, 3, Ok(()));
    assert_eq!(app.approvals_to_run(), [6]);
    assert!(app.approvals_to_run().is_empty());
}

#[test]
fn keys_are_ignored_while_approving() {
    let mut app = app_with_rows(2);
    start_approving(&mut app, 1);
    for code in [KeyCode::Esc, KeyCode::Char('j'), KeyCode::Enter] {
        key(&mut app, code);
    }
    assert!(matches!(app.mode, Mode::Running(_)));
    assert!(!app.should_quit);
}

#[test]
fn q_cancels_the_approvals_that_have_not_started() {
    let mut app = app_with_rows(6);
    start_approving(&mut app, 6);
    assert_eq!(app.approvals_to_run(), [1, 2, 3, 4]);
    approve(&mut app, 1, Ok(()));

    key(&mut app, KeyCode::Char('q'));
    let Mode::Results(summary) = &app.mode else {
        panic!("the results aren't shown");
    };
    assert!(!app.should_quit);
    assert_eq!((summary.approved, summary.total), (1, 6));
    // 2 to 4 were running; 5 and 6 never started.
    assert_eq!(summary.interrupted, [2, 3, 4]);
    assert_eq!(summary.cancelled, 2);
    assert!(summary.failures.is_empty());
    // Nothing more starts, and the batch's rows are refreshed.
    assert!(app.approvals_to_run().is_empty());
    assert_eq!(app.to_refresh, [1, 2, 3, 4, 5, 6]);
    assert_eq!(app.selected.len(), 6);
    // A result that arrives after the cancel changes nothing.
    approve(&mut app, 2, Ok(()));
    assert!(matches!(app.mode, Mode::Results(_)));
}

#[test]
fn a_result_from_a_cancelled_batch_does_not_touch_the_next_one() {
    let mut app = app_with_rows(2);
    start_approving(&mut app, 1);
    assert_eq!(app.approvals_to_run(), [1]);
    let old = app.batch_epoch();
    key(&mut app, KeyCode::Char('q'));
    key(&mut app, KeyCode::Enter);

    // The same PR is approved again, and then the first batch's approval comes back. It is
    // still selected from the first batch.
    ctrl_g(&mut app);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Running(_)));
    assert_eq!(app.approvals_to_run(), [1]);
    app.update(Msg::Approved(old, 1, Ok(())));
    let Mode::Running(batch) = &app.mode else {
        panic!("the new batch was ended by an old result");
    };
    assert_eq!(batch.items[0].approval, ItemStatus::Working);
    approve(&mut app, 1, Ok(()));
    assert!(matches!(app.mode, Mode::Results(_)));
}

#[test]
fn q_while_approving_to_merge_cancels_the_merges_too() {
    let mut app = start_approving_and_merging();
    app.approvals_to_run();
    approve(&mut app, 1, Ok(()));
    approve(&mut app, 2, Ok(()));

    key(&mut app, KeyCode::Char('q'));
    assert_eq!(app.merge_to_run(), None);
    let Mode::Results(summary) = &app.mode else {
        panic!("the results aren't shown");
    };
    assert_eq!(summary.approved, 2);
    assert_eq!(summary.interrupted, [3, 4]);
    // PRs 1 and 2 were approved but their merges didn't start.
    assert_eq!(summary.cancelled, 2);
    assert_eq!(summary.merged, Some((0, 2)));
}

#[test]
fn q_while_merging_cancels_the_merges_not_yet_started() {
    let mut app = start_approving_and_merging();
    app.approvals_to_run();
    for number in 1..=4 {
        approve(&mut app, number, Ok(()));
    }
    assert_eq!(app.merge_to_run(), Some((1, false)));
    merge(&mut app, 1, Ok(()));
    assert_eq!(app.merge_to_run(), Some((2, true)));

    key(&mut app, KeyCode::Char('q'));
    assert_eq!(app.merge_to_run(), None);
    let Mode::Results(summary) = &app.mode else {
        panic!("the results aren't shown");
    };
    assert_eq!(summary.approved, 4);
    assert_eq!(summary.merged, Some((1, 4)));
    assert_eq!(summary.interrupted, [2]);
    assert_eq!(summary.cancelled, 2);
    assert_eq!(app.refreshes_to_fetch(), [1, 2, 3, 4]);
}

#[test]
fn the_last_approval_shows_results_and_refreshes_the_batch() {
    let mut app = app_with_rows(4);
    start_approving(&mut app, 3);
    app.approvals_to_run();
    approve(&mut app, 1, Ok(()));
    approve(&mut app, 3, Err(color_eyre::eyre::eyre!("no\npermission")));
    assert!(matches!(app.mode, Mode::Running(_)));
    assert!(app.refreshes_to_fetch().is_empty());

    approve(&mut app, 2, Ok(()));
    let Mode::Results(summary) = &app.mode else {
        panic!("the results aren't shown");
    };
    assert_eq!((summary.approved, summary.total), (2, 3));
    assert_eq!(
        summary.failures,
        [(3, "approve", "no permission".to_string())]
    );
    assert_eq!(summary.merged, None);
    assert_eq!(app.selected.len(), 3);
    // Only the batch's rows are fetched again, not the whole search.
    assert_eq!(app.list_to_fetch(), None);
    assert_eq!(app.refreshes_to_fetch(), [1, 2, 3]);

    key(&mut app, KeyCode::Char('q'));
    assert!(matches!(app.mode, Mode::List));
    assert!(!app.should_quit);
}

/// An open PR whose checks are passing, pending or absent.
fn pr_with_checks(number: u64, checks: &str) -> Pr {
    let rollup = match checks {
        "passing" => serde_json::json!([{
            "__typename": "CheckRun", "name": "build", "workflowName": "CI",
            "status": "COMPLETED", "conclusion": "SUCCESS", "startedAt": null,
        }]),
        "pending" => serde_json::json!([{
            "__typename": "StatusContext", "context": "ci", "state": "PENDING",
        }]),
        _ => serde_json::json!([]),
    };
    pr_with(number, "t", "OPEN", "", "MERGEABLE", rollup)
}

/// Lists PRs 1 to 4 (passing, pending, no checks, pending), selects them all and chooses
/// Approve & merge.
fn start_approving_and_merging() -> App {
    let prs = vec![
        pr_with_checks(1, "passing"),
        pr_with_checks(2, "pending"),
        pr_with_checks(3, "none"),
        pr_with_checks(4, "pending"),
    ];
    let mut app = app_with(prs);
    assert_eq!(numbers(&app), [1, 2, 3, 4]);
    press(&mut app, " j j j ");
    ctrl_g(&mut app);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Running(_)));
    app
}

fn merge(app: &mut App, number: u64, result: Result<()>) {
    let epoch = app.batch_epoch();
    app.update(Msg::Merged(epoch, number, result));
}

#[test]
fn approve_and_merge_merges_approved_prs_one_at_a_time() {
    let mut app = start_approving_and_merging();
    assert_eq!(app.approvals_to_run(), [1, 2, 3, 4]);
    approve(&mut app, 1, Ok(()));
    approve(&mut app, 2, Ok(()));
    approve(&mut app, 3, Ok(()));
    approve(&mut app, 4, Err(color_eyre::eyre::eyre!("own PR")));

    // Only the approved PRs are merged, one at a time. Pending checks use auto-merge; no
    // checks don't.
    let Mode::Running(batch) = &app.mode else {
        panic!("the batch stopped before merging");
    };
    assert_eq!(batch.progress(), (1, 4));
    assert_eq!(app.merge_to_run(), Some((1, false)));
    assert_eq!(app.merge_to_run(), None);
    merge(&mut app, 1, Ok(()));
    assert_eq!(app.merge_to_run(), Some((2, true)));
    merge(&mut app, 2, Ok(()));
    assert_eq!(app.merge_to_run(), Some((3, false)));
    merge(&mut app, 3, Err(color_eyre::eyre::eyre!("not mergeable")));
    assert_eq!(app.merge_to_run(), None);

    let Mode::Results(summary) = &app.mode else {
        panic!("the results aren't shown");
    };
    assert_eq!((summary.approved, summary.total), (3, 4));
    // Auto-merge counts as merged.
    assert_eq!(summary.merged, Some((2, 3)));
    assert_eq!(summary.auto_merge, [2]);
    assert_eq!(
        summary.failures,
        [
            (3, "merge", "not mergeable".to_string()),
            (4, "approve", "own PR".to_string()),
        ]
    );
    assert_eq!(app.selected.len(), 4);
    assert_eq!(app.refreshes_to_fetch(), [1, 2, 3, 4]);
}

#[test]
fn an_approved_pr_merges_while_others_are_still_being_approved() {
    let mut app = start_approving_and_merging();
    assert_eq!(app.approvals_to_run(), [1, 2, 3, 4]);
    approve(&mut app, 2, Ok(()));
    assert_eq!(app.merge_to_run(), Some((2, true)));
    // Approvals carry on, but only one merge runs at a time.
    approve(&mut app, 1, Ok(()));
    assert_eq!(app.merge_to_run(), None);
    merge(&mut app, 2, Ok(()));
    assert_eq!(app.merge_to_run(), Some((1, false)));
    merge(&mut app, 1, Ok(()));
    assert!(matches!(app.mode, Mode::Running(_)));

    // The batch ends once the last approval is in and its merge is done.
    approve(&mut app, 3, Ok(()));
    approve(&mut app, 4, Err(color_eyre::eyre::eyre!("own PR")));
    assert!(matches!(app.mode, Mode::Running(_)));
    assert_eq!(app.merge_to_run(), Some((3, false)));
    merge(&mut app, 3, Ok(()));
    let Mode::Results(summary) = &app.mode else {
        panic!("the results aren't shown");
    };
    assert_eq!(summary.merged, Some((3, 3)));
}

#[test]
fn approve_and_merge_with_no_approvals_skips_merging() {
    let mut app = start_approving_and_merging();
    app.approvals_to_run();
    for number in 1..=4 {
        approve(&mut app, number, Err(color_eyre::eyre::eyre!("no")));
    }
    assert_eq!(app.merge_to_run(), None);
    let Mode::Results(summary) = &app.mode else {
        panic!("the results aren't shown");
    };
    assert_eq!(summary.merged, Some((0, 0)));
}

#[test]
fn refreshed_rows_update_in_place() {
    let mut app = app_with_rows(3);
    app.table.select(Some(1));
    app.to_refresh = vec![1, 2, 3];
    assert_eq!(app.refreshes_to_fetch(), [1, 2, 3]);
    assert!(app.refreshing());

    // A merged PR would sort last, but the rows keep their order.
    let mut merged = pr(1, "MERGED");
    merged.review_decision = ReviewDecision::Approved;
    app.update(Msg::PrRefreshed(1, Ok(merged)));
    app.update(Msg::PrRefreshed(2, Err(color_eyre::eyre::eyre!("failed"))));
    app.update(Msg::PrRefreshed(9, Ok(pr(9, "OPEN"))));
    assert!(app.refreshing());
    app.update(Msg::PrRefreshed(
        3,
        Err(color_eyre::eyre::eyre!("timed out")),
    ));
    assert!(!app.refreshing());

    assert_eq!(numbers(&app), [1, 2, 3]);
    assert_eq!(app.rows[0].pr.state, PrState::Merged);
    assert_eq!(app.rows[0].pr.review_decision, ReviewDecision::Approved);
    assert_eq!(app.table.selected(), Some(1));
    // Failed refreshes are reported, and the rows keep their old state.
    assert_eq!(
        app.errors.text().as_deref(),
        Some("Refreshing #2 failed: failed\nRefreshing #3 failed: timed out")
    );
    assert_eq!(app.rows[1].pr.state, PrState::Open);
}

#[test]
fn refresh_failures_are_listed_few_at_a_time_after_a_search_failure() {
    let mut app = app_with_rows(5);
    app.to_refresh = (1..=5).collect();
    app.refreshes_to_fetch();
    app.update(Msg::PrRefreshed(1, Err(color_eyre::eyre::eyre!("a\nb"))));
    for number in 2..=5 {
        app.update(Msg::PrRefreshed(
            number,
            Err(color_eyre::eyre::eyre!("failed")),
        ));
    }
    // A failed search leaves the refresh failures, which are about the rows still shown.
    app.errors.search = Some("Search failed: bad query".to_string());
    let text = app.errors.text().unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0], "Search failed: bad query");
    // Each failure takes one line, however many lines the error has.
    assert_eq!(lines[1], "Refreshing #1 failed: a b");
    assert_eq!(lines[4], "… and 2 more refreshes failed");

    // New results make them all stale.
    let (id, _) = {
        app.list.query_to_fetch = Some(String::new());
        app.list_to_fetch().unwrap()
    };
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
    assert_eq!(app.errors.text(), None);
}

#[test]
fn refreshes_run_at_most_four_at_once() {
    let mut app = app_with_rows(6);
    app.to_refresh = (1..=6).collect();
    assert_eq!(app.refreshes_to_fetch(), [1, 2, 3, 4]);
    assert!(app.refreshes_to_fetch().is_empty());
    app.update(Msg::PrRefreshed(1, Ok(pr(1, "OPEN"))));
    assert_eq!(app.refreshes_to_fetch(), [5]);
}

#[test]
fn a_failed_search_keeps_the_old_results() {
    let mut app = app_with_rows(3);
    press(&mut app, " ");
    press(&mut app, "/");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(
        id,
        Err(color_eyre::eyre::eyre!("bad query")),
    ));
    assert_eq!(
        app.errors.search.as_deref(),
        Some("Search failed: bad query")
    );
    assert!(!app.list.searching);
    assert_eq!(numbers(&app), [1, 2, 3]);
    assert_eq!(app.selected, HashSet::from([1]));
}

fn finish(app: &mut App, number: u64) {
    app.details_loaded(number, Err(color_eyre::eyre::eyre!("failed")));
}

#[test]
fn prefetches_details_from_the_highlight_within_the_limit() {
    let mut app = app_with_rows(8);
    assert_eq!(started(app.details_to_fetch()), [1, 2, 3, 4]);
    // Every slot is busy.
    assert!(app.details_to_fetch().is_empty());

    // Freed slots go to the highlighted row and those below it, then wrap to the top.
    app.table.select(Some(5));
    finish(&mut app, 1);
    assert_eq!(started(app.details_to_fetch()), [6]);
    finish(&mut app, 2);
    finish(&mut app, 3);
    assert_eq!(started(app.details_to_fetch()), [7, 8]);
    finish(&mut app, 4);
    assert_eq!(started(app.details_to_fetch()), [5]);

    // Nothing is fetched twice, including failures.
    for number in 5..=8 {
        finish(&mut app, number);
    }
    assert!(app.details_to_fetch().is_empty());
}

#[test]
fn prefetches_diffs_from_the_highlight_within_the_limit() {
    let mut app = app_with_rows(8);
    assert_eq!(started(app.diffs_to_fetch()), [1, 2, 3, 4]);
    // Every slot is busy.
    assert!(app.diffs_to_fetch().is_empty());

    // Freed slots go to the highlighted row and those below it, then wrap to the top.
    app.table.select(Some(5));
    app.diff_loaded(1, Ok("diff\t".to_string()));
    assert!(matches!(&app.diffs[&1], Fetch::Loaded(d) if d.text.to_string() == "diff    "));
    assert_eq!(started(app.diffs_to_fetch()), [6]);
    for number in [2, 3] {
        app.diff_loaded(number, Ok(String::new()));
    }
    assert_eq!(started(app.diffs_to_fetch()), [7, 8]);
    app.diff_loaded(4, Ok(String::new()));
    assert_eq!(started(app.diffs_to_fetch()), [5]);

    // Nothing is fetched twice, including failures.
    for number in 5..=8 {
        app.diff_loaded(number, Err(color_eyre::eyre::eyre!("failed")));
    }
    assert!(app.diffs_to_fetch().is_empty());
}

#[test]
fn diff_colours_become_styles() {
    let mut app = app_with_rows(1);
    app.diff_loaded(1, Ok("\x1b[32m+added\x1b[m\n ctx".to_string()));
    let Fetch::Loaded(diff) = &app.diffs[&1] else {
        panic!("diff should be loaded");
    };
    assert_eq!(diff.text.to_string(), "+added\n ctx");
    // The widest line is worked out when it loads.
    assert_eq!(diff.widest, 6);
    assert_eq!(
        diff.text.lines[0].spans[0].style.fg,
        Some(ratatui::style::Color::Green)
    );
}

/// Presses Tab until `pane` has focus.
fn focus(app: &mut App, pane: Pane) {
    for _ in 0..5 {
        if app.focus == pane {
            return;
        }
        key(app, KeyCode::Tab);
    }
    panic!("Tab never reached the pane");
}

#[test]
fn tab_cycles_focus_and_arrows_act_on_the_focused_pane() {
    let mut app = app_with_rows(3);
    app.drawn.page = 5;
    assert!(app.focus == Pane::Table);

    // The table moves its highlight; a page is clamped to the last row.
    key(&mut app, KeyCode::Down);
    assert_eq!(app.table.selected(), Some(1));
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.table.selected(), Some(2));

    key(&mut app, KeyCode::Tab);
    assert!(app.focus == Pane::Checks);
    app.drawn.checks_layout = ChecksLayout {
        count: 4,
        first: 0,
        viewport: 2,
    };
    press(&mut app, "jjj");
    assert_eq!((app.scroll.check, app.scroll.checks), (3, 2));
    key(&mut app, KeyCode::PageUp);
    assert_eq!((app.scroll.check, app.scroll.checks), (3, 0));

    // In the comments pane the arrows go comment by comment, and a page scrolls.
    key(&mut app, KeyCode::Tab);
    assert!(app.focus == Pane::Comments);
    app.drawn.comment_layout = CommentLayout {
        items: vec![(0, 1), (1, 1), (2, 1)],
        viewport: 5,
    };
    press(&mut app, "jj");
    assert_eq!(app.scroll.comment, 2);
    key(&mut app, KeyCode::PageDown);
    assert_eq!((app.scroll.comment, app.scroll.comments), (2, 5));
    key(&mut app, KeyCode::PageUp);
    key(&mut app, KeyCode::PageUp);
    assert_eq!(app.scroll.comments, 0);
    press(&mut app, "k");
    assert_eq!((app.scroll.comment, app.scroll.comments), (1, 0));

    key(&mut app, KeyCode::Tab);
    assert!(app.focus == Pane::Description);
    press(&mut app, "jj");
    key(&mut app, KeyCode::PageDown);
    assert_eq!(app.scroll.description, 7);
    key(&mut app, KeyCode::PageUp);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.scroll.description, 1);
    // The highlight stayed where it was, and the other panes didn't scroll.
    assert_eq!(app.table.selected(), Some(2));
    assert_eq!((app.scroll.checks, app.scroll.diff), (0, 0));
    key(&mut app, KeyCode::PageUp);
    assert_eq!(app.scroll.description, 0);

    // The approvers, above the diff, scroll a line at a time.
    key(&mut app, KeyCode::Tab);
    assert!(app.focus == Pane::Approvers);
    press(&mut app, "jj");
    assert_eq!(app.scroll.approvers, 2);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.scroll.approvers, 1);
    key(&mut app, KeyCode::Home);
    assert_eq!(app.scroll.approvers, 0);
    // Left and right have nothing to scroll there.
    press(&mut app, "l");

    key(&mut app, KeyCode::Tab);
    assert!(app.focus == Pane::Diff);
    press(&mut app, "k");
    key(&mut app, KeyCode::Down);
    assert_eq!(app.scroll.diff, 1);

    key(&mut app, KeyCode::Tab);
    assert!(app.focus == Pane::Table);
    key(&mut app, KeyCode::BackTab);
    assert!(app.focus == Pane::Diff);
}

#[test]
fn down_and_up_scroll_a_tall_comment_before_moving_to_the_next() {
    let mut app = app_with_rows(1);
    // A one line comment, an open one ten lines tall, and another one line comment.
    app.drawn.comment_layout = CommentLayout {
        items: vec![(0, 1), (1, 10), (11, 1)],
        viewport: 5,
    };
    focus(&mut app, Pane::Comments);
    let state = |app: &App| (app.scroll.comment, app.scroll.comments);

    // The tall comment's header is on screen, so the first step moves to it.
    press(&mut app, "j");
    assert_eq!(state(&app), (1, 0));
    // The last comment's header is at line 11, so it takes seven lines of scrolling before
    // the header is on screen, and one more step to move to it.
    press(&mut app, "jjjjjjj");
    assert_eq!(state(&app), (1, 7));
    press(&mut app, "j");
    assert_eq!(state(&app), (2, 7));

    // Back up: as soon as the last header is off the bottom the tall comment, which is the
    // one on screen, is highlighted. Its predecessor's header is at line 0, so scroll up to it.
    press(&mut app, "k");
    assert_eq!(state(&app), (1, 6));
    press(&mut app, "kkkkk");
    assert_eq!(state(&app), (1, 1));
    press(&mut app, "k");
    assert_eq!(state(&app), (1, 0));
    press(&mut app, "k");
    assert_eq!(state(&app), (0, 0));
    // A page scrolls the view and leaves the highlight where it was, even off screen.
    app.drawn.page = 4;
    app.scroll.comment = 0;
    app.scroll.comments = 0;
    key(&mut app, KeyCode::PageDown);
    assert_eq!(state(&app), (0, 4));
    app.scroll.comment = 2;
    app.scroll.comments = 7;
    key(&mut app, KeyCode::PageUp);
    assert_eq!(state(&app), (2, 3));
    // The next step brings the highlight back to a header on screen.
    press(&mut app, "k");
    assert_eq!(state(&app), (1, 2));
}

#[test]
fn the_wheel_steps_through_the_comments_like_down_and_up() {
    let mut app = app_with_rows(1);
    app.drawn.comment_layout = CommentLayout {
        items: vec![(0, 1), (1, 10), (11, 1)],
        viewport: 5,
    };
    let state = |app: &App| (app.scroll.comment, app.scroll.comments);
    app.wheel(Pane::Comments, 1);
    assert_eq!(state(&app), (1, 0));
    // The next header is off screen, so it scrolls a line at a time, as Down does.
    app.wheel(Pane::Comments, 1);
    assert_eq!(state(&app), (1, 1));
    app.wheel(Pane::Comments, -1);
    assert_eq!(state(&app), (1, 0));
    app.wheel(Pane::Comments, -1);
    assert_eq!(state(&app), (0, 0));
}

#[test]
fn home_and_end_jump_to_the_first_and_last_item_or_the_ends_of_a_pane() {
    let mut app = app_with_rows(3);
    key(&mut app, KeyCode::End);
    assert_eq!(app.table.selected(), Some(2));
    key(&mut app, KeyCode::Home);
    assert_eq!(app.table.selected(), Some(0));

    // In the comments the highlight goes to the first or last comment, with the view.
    app.drawn.comment_layout = CommentLayout {
        items: vec![(0, 1), (1, 10), (11, 1)],
        viewport: 5,
    };
    focus(&mut app, Pane::Comments);
    key(&mut app, KeyCode::End);
    assert_eq!((app.scroll.comment, app.scroll.comments), (2, 11));
    key(&mut app, KeyCode::Home);
    assert_eq!((app.scroll.comment, app.scroll.comments), (0, 0));

    // Elsewhere the scroll goes to the top, and to the bottom once the view clamps it.
    focus(&mut app, Pane::Description);
    key(&mut app, KeyCode::End);
    assert_eq!(app.scroll.description, u16::MAX);
    key(&mut app, KeyCode::Home);
    assert_eq!(app.scroll.description, 0);
    focus(&mut app, Pane::Diff);
    key(&mut app, KeyCode::End);
    assert_eq!(app.scroll.diff, u16::MAX);
    // In the checks too, and the view follows the highlight.
    app.drawn.checks_layout = ChecksLayout {
        count: 5,
        first: 0,
        viewport: 3,
    };
    focus(&mut app, Pane::Checks);
    key(&mut app, KeyCode::End);
    assert_eq!((app.scroll.check, app.scroll.checks), (4, 2));
    key(&mut app, KeyCode::Home);
    assert_eq!((app.scroll.check, app.scroll.checks), (0, 0));
}

#[test]
fn up_and_down_move_the_checks_highlight_and_keep_it_on_screen() {
    let mut app = app_with_rows(1);
    // A conflict row above five checks, three rows high.
    app.drawn.checks_layout = ChecksLayout {
        count: 5,
        first: 1,
        viewport: 3,
    };
    focus(&mut app, Pane::Checks);
    let state = |app: &App| (app.scroll.check, app.scroll.checks);
    press(&mut app, "j");
    assert_eq!(state(&app), (1, 0));
    // Check 2 is at row 3, just below the pane: the view scrolls one row.
    press(&mut app, "j");
    assert_eq!(state(&app), (2, 1));
    press(&mut app, "jjjj");
    assert_eq!(state(&app), (4, 3));
    // Back up: the highlight leaves the view's edge only when it has to.
    press(&mut app, "kk");
    assert_eq!(state(&app), (2, 3));
    press(&mut app, "k");
    assert_eq!(state(&app), (1, 2));
    // The first check also shows the conflict row above it.
    press(&mut app, "k");
    assert_eq!(state(&app), (0, 0));
    // The wheel steps like Down.
    app.wheel(Pane::Checks, 1);
    assert_eq!(state(&app), (1, 0));
}

#[test]
fn enter_and_space_expand_the_highlighted_comment_instead_of_reviewing() {
    let mut app = app_with_rows(2);
    app.drawn.comment_layout = CommentLayout {
        items: vec![(0, 1), (1, 1)],
        viewport: 5,
    };
    focus(&mut app, Pane::Comments);
    press(&mut app, "j");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::List));
    assert!(app.scroll.comments_open.contains(&1));
    press(&mut app, " ");
    assert!(app.scroll.comments_open.is_empty());
    assert!(app.selected.is_empty());
}

#[test]
fn scroll_goes_back_to_the_top_when_the_highlight_moves() {
    let mut app = app_with_rows(3);
    for (pane, keys) in [
        (Pane::Checks, "j"),
        (Pane::Comments, "jj"),
        (Pane::Description, "jj"),
        (Pane::Diff, "jjj"),
    ] {
        focus(&mut app, pane);
        press(&mut app, keys);
    }
    focus(&mut app, Pane::Table);
    key(&mut app, KeyCode::Down);
    assert_eq!(
        (app.scroll.checks, app.scroll.description, app.scroll.diff),
        (0, 0, 0)
    );
    assert_eq!((app.scroll.comment, app.scroll.comments), (0, 0));

    // A highlight that can't move, at the last row, leaves the scroll alone.
    key(&mut app, KeyCode::Down);
    focus(&mut app, Pane::Description);
    press(&mut app, "j");
    focus(&mut app, Pane::Table);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.scroll.description, 1);
}

fn limited_app(limit: u32) -> App {
    App::new("PRs".to_string(), "is:pr".to_string(), false).with_limit(limit)
}

#[test]
fn a_list_longer_than_the_limit_is_cut_and_the_title_says_so() {
    let mut app = limited_app(2);
    let (id, _) = app.list_to_fetch().unwrap();
    // GitHub was asked for 3, one more than the limit, and returned all of them.
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=3))));
    assert_eq!(numbers(&app), [1, 2]);
    assert_eq!(app.title(), "PRs - showing first 2, more exist");
}

#[test]
fn a_list_exactly_the_limit_has_no_note() {
    let mut app = limited_app(2);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
    assert_eq!(numbers(&app), [1, 2]);
    assert_eq!(app.title(), "PRs");
}

#[test]
fn the_note_follows_the_latest_list_and_survives_a_failed_search() {
    let mut app = limited_app(1);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
    assert!(app.title().ends_with("more exist"));

    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Err(color_eyre::eyre::eyre!("boom"))));
    assert!(app.title().ends_with("more exist"));

    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=1))));
    assert_eq!(app.title(), "PRs");
}

fn ctrl_r(app: &mut App) {
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Char('r'),
        KeyModifiers::CONTROL,
    )));
}

#[test]
fn ctrl_r_fetches_the_submitted_query() {
    let mut app = app_with_rows(3);
    ctrl_r(&mut app);
    let (_, query) = app.list_to_fetch().unwrap();
    assert_eq!(query, "is:pr");
}

#[test]
fn ctrl_r_is_ignored_while_a_search_runs_and_under_dialogs() {
    let mut app = app_with_rows(3);
    ctrl_r(&mut app);
    assert!(app.list_to_fetch().is_some());
    // The fetch is running.
    ctrl_r(&mut app);
    assert_eq!(app.list_to_fetch(), None);

    let mut app = app_with_rows(3);
    press(&mut app, "?");
    ctrl_r(&mut app);
    assert_eq!(app.list_to_fetch(), None);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Confirm(_)));
    ctrl_r(&mut app);
    assert_eq!(app.list_to_fetch(), None);
}

#[test]
fn ctrl_r_keeps_the_selection_by_number_when_rows_move() {
    let mut app = app_with_rows(3);
    press(&mut app, " jj ");
    assert_eq!(app.selected, HashSet::from([1, 3]));
    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    // 3 moves to the top, 1 is gone and 4 is new.
    app.update(Msg::PrsLoaded(id, Ok(prs([3, 2, 4]))));
    assert_eq!(numbers(&app), [3, 2, 4]);
    assert_eq!(app.selected, HashSet::from([3]));
    assert_eq!(app.hidden_selected(), 0);
}

#[test]
fn a_new_search_still_clears_the_selection() {
    let mut app = app_with_rows(3);
    press(&mut app, " ");
    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs([1, 2, 3]))));
    assert!(app.selected.is_empty());
}

#[test]
fn ctrl_r_keeps_the_highlight_on_the_same_pr_and_clears_the_filter() {
    let mut app = app_with_titles();
    press(&mut app, "jfbeta");
    key(&mut app, KeyCode::Enter);
    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(
        id,
        Ok(vec![titled(3, "c"), titled(2, "Bump beta"), titled(1, "a")]),
    ));
    assert_eq!(app.filter.value(), "");
    assert_eq!(visible_numbers(&app), [3, 2, 1]);
    assert_eq!(app.highlighted().unwrap().pr.number, 2);

    // If it is gone, the first row is highlighted.
    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(vec![titled(8, "x"), titled(9, "y")])));
    assert_eq!(app.highlighted().unwrap().pr.number, 8);
}

#[test]
fn ctrl_r_drops_cached_details_and_diffs_but_a_new_search_keeps_them() {
    let mut app = app_with_rows(2);
    app.details_to_fetch();
    app.diffs_to_fetch();
    assert_eq!(app.details.len(), 2);

    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
    assert_eq!((app.details.len(), app.diffs.len()), (2, 2));

    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
    assert_eq!((app.details.len(), app.diffs.len()), (0, 0));
    // So they are fetched again.
    assert_eq!(started(app.details_to_fetch()), [1, 2]);
}

fn details_with_body(body: &str) -> PrDetails {
    serde_json::from_value(serde_json::json!({
        "headRefName": "branch",
        "body": body,
        "comments": [],
        "additions": 1,
        "deletions": 1,
    }))
    .unwrap()
}

/// The body of the loaded details of a PR, if they are loaded.
fn loaded_body(app: &App, number: u64) -> Option<String> {
    match app.details.get(&number) {
        Some(Fetch::Loaded(loaded)) => loaded.details.body.clone(),
        _ => None,
    }
}

#[test]
fn a_result_from_before_ctrl_r_is_dropped() {
    let mut app = app_with_rows(2);
    let details = app.details_to_fetch();
    let diffs = app.diffs_to_fetch();
    assert_eq!(started(details.clone()), [1, 2]);

    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
    assert!(app.details.is_empty() && app.diffs.is_empty());

    // The fetches that were running now finish, with what was current before the refresh.
    for &(number, id) in &details {
        app.update(Msg::DetailsLoaded(number, id, Ok(details_with_body("old"))));
    }
    for &(number, id) in &diffs {
        app.update(Msg::DiffLoaded(number, id, Ok("old".to_string())));
    }
    assert!(app.details.is_empty() && app.diffs.is_empty());
    // Failures from them are dropped too.
    let (number, id) = details[0];
    app.update(Msg::DetailsLoaded(
        number,
        id,
        Err(color_eyre::eyre::eyre!("old failure")),
    ));
    assert!(app.details.is_empty());
}

#[test]
fn a_result_from_before_ctrl_r_does_not_replace_the_new_fetch() {
    let mut app = app_with_rows(1);
    let before = app.details_to_fetch()[0].1;

    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=1))));
    let after = app.details_to_fetch()[0].1;
    assert_ne!(before, after);

    // The old fetch finishes first and is ignored, so the PR is still loading.
    app.update(Msg::DetailsLoaded(1, before, Ok(details_with_body("old"))));
    assert!(matches!(app.details[&1], Fetch::Loading(current) if current == after));
    app.update(Msg::DetailsLoaded(1, after, Ok(details_with_body("new"))));
    assert_eq!(loaded_body(&app, 1).as_deref(), Some("new"));
    // A late duplicate doesn't overwrite what loaded.
    app.update(Msg::DetailsLoaded(1, before, Ok(details_with_body("old"))));
    assert_eq!(loaded_body(&app, 1).as_deref(), Some("new"));
}

#[test]
fn a_result_from_before_a_changed_pr_was_dropped_is_ignored() {
    let mut app = app_with_rows(2);
    let details = app.details_to_fetch();
    let diffs = app.diffs_to_fetch();
    let fetch = start_status_refresh(&mut app).unwrap();
    let update = update(vec![
        status(1, "OPEN", false, "", "2026-02-02T00:00:00Z"),
        status(2, "OPEN", true, "", UNCHANGED),
    ]);
    app.update(Msg::StatusRefreshed(fetch.id, update));
    let fresh = started(app.details_to_fetch());
    assert_eq!(fresh, [1]);

    // The details of #1 that were running are stale; those of #2 are not.
    for &(number, id) in &details {
        app.update(Msg::DetailsLoaded(number, id, Ok(details_with_body("d"))));
    }
    assert!(matches!(app.details[&1], Fetch::Loading(_)));
    assert_eq!(loaded_body(&app, 2).as_deref(), Some("d"));
    for &(number, id) in &diffs {
        app.update(Msg::DiffLoaded(number, id, Ok(String::new())));
    }
    assert!(!app.diffs.contains_key(&1));
    assert!(matches!(app.diffs[&2], Fetch::Loaded(_)));
}

#[test]
fn a_failed_ctrl_r_keeps_the_list_and_the_selection() {
    let mut app = app_with_rows(3);
    press(&mut app, " ");
    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Err(color_eyre::eyre::eyre!("boom"))));
    assert_eq!(numbers(&app), [1, 2, 3]);
    assert_eq!(app.selected, HashSet::from([1]));
    assert!(app.errors.text().unwrap().contains("boom"));
}

fn status(number: u64, state: &str, passing: bool, review: &str, updated: &str) -> PrStatus {
    let rollup = if passing {
        serde_json::json!([{
            "__typename": "CheckRun", "name": "build", "workflowName": "CI",
            "status": "COMPLETED", "conclusion": "SUCCESS", "startedAt": null,
        }])
    } else {
        serde_json::json!([])
    };
    serde_json::from_value(serde_json::json!({
        "number": number,
        "state": state,
        "statusCheckRollup": rollup,
        "mergeable": "MERGEABLE",
        "reviewDecision": review,
        "updatedAt": updated,
    }))
    .unwrap()
}

fn update(statuses: Vec<PrStatus>) -> StatusUpdate {
    StatusUpdate {
        statuses,
        failure: None,
    }
}

const UNCHANGED: &str = "2026-01-01T00:00:00Z";

/// Starts a timed refresh, as the timer and the loop would.
fn start_status_refresh(app: &mut App) -> Option<StatusFetch> {
    app.update(Msg::RefreshDue);
    app.status_to_fetch()
}

#[test]
fn a_timed_refresh_covers_the_open_rows_once_at_a_time() {
    let mut app = App::new(String::new(), "is:pr".to_string(), false);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(
        id,
        Ok(vec![pr(1, "OPEN"), pr(2, "MERGED"), pr(3, "OPEN")]),
    ));
    let fetch = start_status_refresh(&mut app).unwrap();
    assert_eq!(fetch.query, "is:pr");
    assert_eq!(fetch.candidates, [1, 3]);
    // One at a time.
    assert!(start_status_refresh(&mut app).is_none());
    app.update(Msg::StatusRefreshed(fetch.id, update(vec![])));
    assert!(start_status_refresh(&mut app).is_some());
}

#[test]
fn a_timed_refresh_that_came_due_out_of_focus_runs_when_focus_returns() {
    let mut app = app_with_rows(2);
    app.update(Msg::FocusLost);
    assert!(app.status.paused());
    // The timer keeps firing, and nothing starts.
    assert!(start_status_refresh(&mut app).is_none());
    assert!(start_status_refresh(&mut app).is_none());
    // Back in focus, the missed one runs at once, with no wait for the next tick.
    app.update(Msg::FocusGained);
    assert!(!app.status.paused());
    assert!(app.status_to_fetch().is_some());
    // Only one, and nothing is owed afterwards.
    assert!(app.status_to_fetch().is_none());
}

#[test]
fn losing_and_regaining_focus_without_a_missed_refresh_starts_none() {
    let mut app = app_with_rows(2);
    app.update(Msg::FocusLost);
    app.update(Msg::FocusGained);
    assert!(app.status_to_fetch().is_none());
    // The timer works as before.
    assert!(start_status_refresh(&mut app).is_some());
}

#[test]
fn p_pauses_and_resumes_the_timed_refresh() {
    let mut app = app_with_rows(2);
    press(&mut app, "p");
    assert!(app.status.paused());
    assert!(start_status_refresh(&mut app).is_none());
    // Resuming runs the one that was missed.
    press(&mut app, "p");
    assert!(!app.status.paused());
    assert!(app.status_to_fetch().is_some());

    // Pausing with nothing missed doesn't start one on resuming.
    press(&mut app, "pp");
    assert!(app.status_to_fetch().is_none());
}

#[test]
fn a_refresh_stays_paused_until_both_focus_and_the_key_allow_it() {
    let mut app = app_with_rows(2);
    press(&mut app, "p");
    app.update(Msg::FocusLost);
    assert!(start_status_refresh(&mut app).is_none());
    // Focus is back, but `p` still holds it.
    app.update(Msg::FocusGained);
    assert!(app.status.paused());
    assert!(app.status_to_fetch().is_none());
    press(&mut app, "p");
    assert!(app.status_to_fetch().is_some());
}

#[test]
fn ctrl_r_still_runs_the_query_while_paused() {
    let mut app = app_with_rows(2);
    app.update(Msg::FocusLost);
    press(&mut app, "p");
    ctrl_r(&mut app);
    assert!(app.list_to_fetch().is_some());
}

#[test]
fn a_timed_refresh_waits_for_a_search_a_batch_and_row_refreshes() {
    let mut app = app_with_rows(2);
    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    app.list_to_fetch().unwrap();
    assert!(app.list.searching);
    assert!(start_status_refresh(&mut app).is_none());

    let mut app = app_with_rows(2);
    start_approving(&mut app, 1);
    assert!(start_status_refresh(&mut app).is_none());

    let mut app = app_with_rows(2);
    app.to_refresh.push(1);
    assert!(start_status_refresh(&mut app).is_none());

    // Nothing to refresh before a list loads, or when no PR is open.
    let mut app = App::new(String::new(), "is:pr".to_string(), false);
    assert!(start_status_refresh(&mut app).is_none());
    let mut app = app_with(vec![pr(1, "MERGED")]);
    assert!(start_status_refresh(&mut app).is_none());
    assert!(start_status_refresh(&mut app).is_none());
}

#[test]
fn a_timed_refresh_updates_rows_in_place() {
    let mut app = app_with_titles();
    press(&mut app, "jf");
    press(&mut app, "no checks");
    key(&mut app, KeyCode::Enter);
    press(&mut app, " ");
    let fetch = start_status_refresh(&mut app).unwrap();
    let update = update(vec![
        status(2, "OPEN", true, "APPROVED", UNCHANGED),
        status(3, "OPEN", false, "", UNCHANGED),
    ]);
    app.update(Msg::StatusRefreshed(fetch.id, update));

    // Same order, same selection, same highlight, and the filter isn't applied again: the
    // row that now has passing checks stays even though "no checks" no longer matches it.
    assert_eq!(numbers(&app), [1, 2, 3]);
    assert_eq!(app.selected, HashSet::from([2]));
    assert_eq!(visible_numbers(&app), [1, 2, 3]);
    assert_eq!(app.highlighted().unwrap().pr.number, 2);
    let row = &app.rows[1];
    assert_eq!(row.checks.state, CheckState::Passing);
    assert_eq!(row.pr.review_decision, ReviewDecision::Approved);
    assert_eq!(row.pr.title, "Bump beta");
}

#[test]
fn a_pr_that_changed_has_its_details_and_diff_fetched_again() {
    let mut app = app_with_rows(2);
    app.details_to_fetch();
    app.diffs_to_fetch();
    let fetch = start_status_refresh(&mut app).unwrap();
    let update = update(vec![
        status(1, "OPEN", false, "", "2026-02-02T00:00:00Z"),
        status(2, "OPEN", true, "", UNCHANGED),
    ]);
    app.update(Msg::StatusRefreshed(fetch.id, update));
    assert!(!app.details.contains_key(&1) && !app.diffs.contains_key(&1));
    assert!(app.details.contains_key(&2) && app.diffs.contains_key(&2));
    assert_eq!(started(app.details_to_fetch()), [1]);
}

#[test]
fn a_row_known_to_be_closed_is_not_updated() {
    let mut app = app_with(vec![pr(1, "OPEN"), pr(2, "MERGED")]);
    let fetch = start_status_refresh(&mut app).unwrap();
    let update = update(vec![status(
        2,
        "OPEN",
        true,
        "APPROVED",
        "2026-02-02T00:00:00Z",
    )]);
    app.update(Msg::StatusRefreshed(fetch.id, update));
    assert_eq!(app.rows[1].pr.state, PrState::Merged);
    assert_eq!(app.rows[1].pr.review_decision, ReviewDecision::Other);
}

#[test]
fn an_open_row_found_merged_becomes_merged() {
    let mut app = app_with_rows(2);
    let fetch = start_status_refresh(&mut app).unwrap();
    let update = update(vec![status(1, "MERGED", true, "APPROVED", UNCHANGED)]);
    app.update(Msg::StatusRefreshed(fetch.id, update));
    assert_eq!(app.rows[0].pr.state, PrState::Merged);
    assert_eq!(app.rows[0].checks_label(), "· merged");
    // It stays where it was, and is no longer a candidate.
    assert_eq!(numbers(&app), [1, 2]);
    let fetch = start_status_refresh(&mut app).unwrap();
    assert_eq!(fetch.candidates, [2]);
}

#[test]
fn results_for_an_older_list_are_dropped() {
    let mut app = app_with_rows(2);
    let fetch = start_status_refresh(&mut app).unwrap();
    // Ctrl-R replaces the list while the refresh runs.
    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
    let update = update(vec![status(1, "MERGED", true, "APPROVED", UNCHANGED)]);
    app.update(Msg::StatusRefreshed(fetch.id, update));
    assert_eq!(app.rows[0].pr.state, PrState::Open);
    // The slot is free again.
    assert!(start_status_refresh(&mut app).is_some());
}

#[test]
fn results_that_began_before_a_batch_are_dropped() {
    let mut app = app_with_rows(2);
    let fetch = start_status_refresh(&mut app).unwrap();
    start_approving(&mut app, 1);
    approve(&mut app, 1, Ok(()));
    assert!(matches!(app.mode, Mode::Results(_)));
    let update = update(vec![status(1, "OPEN", false, "", "2026-02-02T00:00:00Z")]);
    app.update(Msg::StatusRefreshed(fetch.id, update));
    // The old state didn't go over the row the batch changed.
    assert_eq!(app.rows[0].pr.updated_at, UNCHANGED);
}

#[test]
fn a_failed_refresh_is_shown_until_one_succeeds_and_keeps_what_it_got() {
    let mut app = app_with_rows(2);
    let fetch = start_status_refresh(&mut app).unwrap();
    let failed = StatusUpdate {
        statuses: vec![status(2, "OPEN", true, "", UNCHANGED)],
        failure: Some("boom\nsecond line".to_string()),
    };
    app.update(Msg::StatusRefreshed(fetch.id, failed));
    assert_eq!(app.status.failure.as_deref(), Some("boom second line"));
    assert_eq!(app.rows[1].checks.state, CheckState::Passing);
    // It isn't an error above the status bar.
    assert_eq!(app.errors.text(), None);

    let fetch = start_status_refresh(&mut app).unwrap();
    app.update(Msg::StatusRefreshed(fetch.id, update(vec![])));
    assert_eq!(app.status.failure, None);
}

#[test]
fn labels_are_fetched_once_when_the_search_box_opens_and_branches_when_one_is_typed() {
    use super::work::NamesToFetch;
    let mut app = app_with_rows(1);
    // Not until the box is open.
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    press(&mut app, "/");
    let labels = app.names_to_fetch().labels.unwrap();
    // Nor again, while they load or after.
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    assert!(app.names.labels().is_empty());

    // Branches are left until a `base:` or `head:` value is being typed.
    press(&mut app, " label:x");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    press(&mut app, " -base:");
    let branches = app.names_to_fetch().branches.unwrap();
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    press(&mut app, " head:");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());

    app.update(Msg::LabelsLoaded(labels, Ok(vec!["deps".to_string()])));
    app.update(Msg::BranchesLoaded(branches, Ok(vec!["main".to_string()])));
    assert_eq!(app.names.labels(), ["deps"]);
    assert_eq!(app.names.branches(), ["main"]);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    press(&mut app, "/");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
}

#[test]
fn a_failed_names_fetch_is_not_retried_until_the_next_search() {
    use super::work::NamesToFetch;
    let mut app = app_with_rows(1);
    press(&mut app, "/ head:");
    let started = app.names_to_fetch();
    app.update(Msg::LabelsLoaded(
        started.labels.unwrap(),
        Err(color_eyre::eyre::eyre!("no")),
    ));
    // The other one is unaffected, and a key press doesn't ask again.
    app.update(Msg::BranchesLoaded(
        started.branches.unwrap(),
        Err(color_eyre::eyre::eyre!("no")),
    ));
    press(&mut app, "x");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());

    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs([1]))));
    press(&mut app, "/");
    let again = app.names_to_fetch();
    assert!(again.labels.is_some());
    // The branches wait until the cursor is in a `head:` value again, which it is.
    assert!(again.branches.is_some());
}

#[test]
fn a_names_result_that_is_not_the_latest_is_dropped() {
    let mut app = app_with_rows(1);
    press(&mut app, "/");
    let id = app.names_to_fetch().labels.unwrap();
    app.update(Msg::LabelsLoaded(id + 100, Ok(vec!["stale".to_string()])));
    assert!(app.names.labels().is_empty());
    app.update(Msg::LabelsLoaded(id, Ok(vec!["fresh".to_string()])));
    assert_eq!(app.names.labels(), ["fresh"]);
}

/// An app whose search box is open with `typed` added to the query, and whose labels are known.
fn app_completing(typed: &str) -> App {
    let mut app = app_with_rows(2);
    press(&mut app, "/");
    let started = app.names_to_fetch();
    let labels = ["deps", "bug"].map(String::from).to_vec();
    app.update(Msg::LabelsLoaded(started.labels.unwrap(), Ok(labels)));
    press(&mut app, typed);
    app
}

fn suggestions(app: &App) -> Vec<String> {
    app.completion()
        .map(|(c, _)| c.suggestions)
        .unwrap_or_default()
}

#[test]
fn no_suggestions_until_the_query_has_been_changed() {
    let mut app = app_with_rows(2);
    press(&mut app, "/");
    assert!(app.completion().is_none());
    press(&mut app, " is:p");
    assert!(app.completion().is_some());
}

#[test]
fn authors_come_from_the_rows_listed() {
    let app = app_completing(" author:");
    assert_eq!(suggestions(&app), ["alice"]);
}

#[test]
fn up_goes_through_the_history_when_there_are_no_suggestions() {
    let mut app = app_with_rows(2);
    press(&mut app, "/x");
    key(&mut app, KeyCode::Enter);
    press(&mut app, "/y");
    key(&mut app, KeyCode::Enter);
    press(&mut app, "/ la");
    assert!(app.completion().is_some());
    key(&mut app, KeyCode::Up);
    // Moved in the suggestions: the query is not recalled.
    assert_eq!(app.query.value(), "is:prxy la");
}

#[test]
fn recalling_a_query_does_not_open_a_list_that_takes_up_and_down() {
    let mut app = app_with_rows(2);
    // A recalled query that ends in something with suggestions.
    press(&mut app, "/ is:op");
    key(&mut app, KeyCode::Enter);
    press(&mut app, "/ x");
    key(&mut app, KeyCode::Up);
    assert_eq!(app.query.value(), "is:pr is:op");
    assert!(app.completion().is_none());
    key(&mut app, KeyCode::Down);
    assert_eq!(app.query.value(), "is:pr is:op x");
}

#[test]
fn esc_closes_the_list_before_it_cancels_the_edit() {
    let mut app = app_completing(" la");
    key(&mut app, KeyCode::Esc);
    assert!(app.completion().is_none());
    assert!(app.typing == Typing::Search);
    assert_eq!(app.query.value(), "is:pr la");
    // It stays closed until the text changes.
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Right);
    assert!(app.completion().is_none());
    press(&mut app, "n");
    assert!(app.completion().is_some());
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    assert!(app.typing != Typing::Search);
    assert_eq!(app.query.value(), "is:pr");
}

#[test]
fn tab_with_no_suggestions_does_nothing_to_the_query() {
    let mut app = app_completing(" zzz");
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.query.value(), "is:pr zzz");
    assert!(app.typing == Typing::Search);
}

#[test]
fn users_and_teams_are_fetched_when_a_qualifier_that_uses_them_is_typed() {
    use super::work::NamesToFetch;
    let mut app = app_with_rows(1);
    press(&mut app, "/");
    app.names_to_fetch();
    // Not for a name that is still being typed, or for a qualifier that doesn't use them.
    press(&mut app, " reviewed-by");
    press(&mut app, " author:");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());

    press(&mut app, " reviewed-by:");
    let started = app.names_to_fetch();
    let users = started.users.unwrap();
    assert!(started.teams.is_none() && started.branches.is_none());
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    press(&mut app, " team-review-requested:");
    let teams = app.names_to_fetch().teams.unwrap();

    app.update(Msg::UsersLoaded(users, Ok(vec!["bob".to_string()])));
    app.update(Msg::TeamsLoaded(teams, Ok(vec!["acme/web".to_string()])));
    assert_eq!(app.names.users(), ["bob"]);
    assert_eq!(app.names.teams(), ["acme/web"]);
    // The users fetched are offered with the authors of the rows.
    key(&mut app, KeyCode::Esc);
    press(&mut app, " review-requested:");
    assert_eq!(suggestions(&app), ["alice", "bob"]);
}

#[test]
fn the_list_opens_with_nothing_chosen_and_enter_still_searches() {
    let mut app = app_completing(" la");
    assert_eq!(suggestions(&app), ["label", "language"]);
    assert_eq!(app.completion().unwrap().1, None);
    key(&mut app, KeyCode::Enter);
    assert!(app.typing != Typing::Search);
    let (_, query) = app.list_to_fetch().unwrap();
    assert_eq!(query, "is:pr la");
}

#[test]
fn tab_and_down_choose_the_first_suggestion_then_the_next_round_the_ends() {
    let chosen = |app: &App| app.completion().unwrap().1;
    for next in [KeyCode::Tab, KeyCode::Down] {
        let mut app = app_completing(" la");
        key(&mut app, next);
        assert_eq!(chosen(&app), Some(0));
        key(&mut app, next);
        assert_eq!(chosen(&app), Some(1));
        key(&mut app, next);
        assert_eq!(chosen(&app), Some(0));
        // Choosing doesn't change the text.
        assert_eq!(app.query.value(), "is:pr la");
    }
}

#[test]
fn shift_tab_and_up_choose_the_first_suggestion_then_the_previous_round_the_ends() {
    let chosen = |app: &App| app.completion().unwrap().1;
    for previous in [KeyCode::BackTab, KeyCode::Up] {
        let mut app = app_completing(" la");
        key(&mut app, previous);
        assert_eq!(chosen(&app), Some(0));
        key(&mut app, previous);
        assert_eq!(chosen(&app), Some(1));
        key(&mut app, KeyCode::Tab);
        assert_eq!(chosen(&app), Some(0));
    }
}

#[test]
fn a_key_that_changes_the_text_takes_the_choice_back() {
    let mut app = app_completing(" la");
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.completion().unwrap().1, Some(0));
    press(&mut app, "n");
    assert_eq!(app.completion().unwrap().1, None);
}

#[test]
fn enter_takes_the_chosen_suggestion_and_does_not_search() {
    let mut app = app_completing(" la");
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.query.value(), "is:pr label:");
    assert_eq!(app.query.cursor(), 12);
    assert!(app.typing == Typing::Search);
    assert_eq!(app.list_to_fetch(), None);
    // Now its values are listed, with nothing chosen, the labels the repository has sorted.
    assert_eq!(suggestions(&app), ["bug", "deps"]);
    assert_eq!(app.completion().unwrap().1, None);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.query.value(), "is:pr label:deps ");
    assert!(app.completion().is_none());
    // With nothing chosen and no list, Enter searches.
    key(&mut app, KeyCode::Enter);
    assert!(app.typing != Typing::Search);
    assert_eq!(app.list_to_fetch().unwrap().1, "is:pr label:deps ");
}

/// A PR with a title, a review decision, a check that passes or not, and when it was updated.
fn sortable(number: u64, title: &str, review: &str, check: &str, updated: &str) -> Pr {
    let rollup = match check {
        "" => serde_json::json!([]),
        state => serde_json::json!([
            {"__typename": "StatusContext", "context": "ci", "state": state}
        ]),
    };
    let mut pr = pr_with(number, title, "OPEN", review, "MERGEABLE", rollup);
    pr.updated_at = format!("2026-01-0{updated}T00:00:00Z");
    pr
}

/// Five open PRs and a merged one, listed in the order 4, 2, 5, 1, 3, 6 and different in every
/// way that can be sorted on.
fn app_to_sort() -> App {
    let mut merged = sortable(6, "Aardvark", "", "SUCCESS", "1");
    merged.state = PrState::Merged;
    app_with(vec![
        sortable(4, "delta", "APPROVED", "FAILURE", "5"),
        sortable(2, "Bravo", "", "PENDING", "2"),
        sortable(5, "echo", "CHANGES_REQUESTED", "SUCCESS", "4"),
        sortable(1, "alpha", "", "SUCCESS", "3"),
        sortable(3, "Charlie", "APPROVED", "SUCCESS", "1"),
        merged,
    ])
}

#[test]
fn the_list_starts_sorted_by_checks_and_s_goes_round_the_sorts() {
    let mut app = app_to_sort();
    // Passing first, then pending, then failing, in the order GitHub listed ties; the merged one
    // is last.
    assert_eq!(numbers(&app), [5, 1, 3, 2, 4, 6]);
    assert_eq!(app.sort(), (Sort::Checks, false));
    let mut seen = Vec::new();
    for _ in 0..6 {
        press(&mut app, "s");
        seen.push(app.sort());
    }
    assert_eq!(
        seen,
        [
            (Sort::Number, false),
            (Sort::Title, false),
            (Sort::Review, false),
            (Sort::Author, false),
            (Sort::Updated, false),
            (Sort::Checks, false),
        ]
    );
}

#[test]
fn shift_s_reverses_the_sort_and_the_next_sort_starts_smallest_first_again() {
    let mut app = app_to_sort();
    press(&mut app, "s");
    assert_eq!(numbers(&app), [1, 2, 3, 4, 5, 6]);
    press(&mut app, "S");
    assert_eq!(app.sort(), (Sort::Number, true));
    // The merged PR is still last: closed PRs stay out of the way whichever way it goes.
    assert_eq!(numbers(&app), [5, 4, 3, 2, 1, 6]);
    press(&mut app, "S");
    assert_eq!(numbers(&app), [1, 2, 3, 4, 5, 6]);
    press(&mut app, "S");
    press(&mut app, "s");
    assert_eq!(app.sort(), (Sort::Title, false));
    assert_eq!(numbers(&app), [1, 2, 3, 4, 5, 6]);
}

#[test]
fn a_reversed_sort_keeps_ties_in_the_order_listed_and_applies_to_the_next_list() {
    let mut app = app_to_sort();
    // Review: those needing one (2, 1), then changes requested (5), then approved (4, 3).
    press(&mut app, "sssS");
    assert_eq!(app.sort(), (Sort::Review, true));
    // Approved first, and each group in the order GitHub listed it.
    assert_eq!(numbers(&app), [4, 3, 5, 2, 1, 6]);
    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(
        id,
        Ok(vec![
            sortable(1, "a", "", "SUCCESS", "1"),
            sortable(2, "b", "APPROVED", "SUCCESS", "1"),
        ]),
    ));
    assert_eq!(numbers(&app), [2, 1]);
    assert_eq!(app.sort(), (Sort::Review, true));
}

#[test]
fn a_title_sort_goes_by_the_title_columns_and_follows_a_change_of_regex() {
    let mut app = app_with(vec![
        titled_for_sort(1, "Bump zeta to 1.0"),
        titled_for_sort(2, "Bump alpha to 3.0"),
        titled_for_sort(3, "Bump alpha to 2.0"),
    ]);
    press(&mut app, "ss");
    assert_eq!(app.sort(), (Sort::Title, false));
    // By the whole title: "Bump alpha to 2.0" < "Bump alpha to 3.0" < "Bump zeta to 1.0".
    assert_eq!(numbers(&app), [3, 2, 1]);
    assert_eq!(app.highlighted().unwrap().pr.number, 1);
    // With a column for the version alone, by that: 1.0, 2.0, 3.0.
    press(&mut app, "c");
    press(&mut app, r"Bump \S+ to (?<version>\S+)$");
    key(&mut app, KeyCode::Enter);
    assert_eq!(numbers(&app), [1, 3, 2]);
    // And the highlight stayed on the PR it was on.
    assert_eq!(app.highlighted().unwrap().pr.number, 1);
}

/// A PR by this author, with nothing else to tell it from the rest.
fn by_author(number: u64, login: &str) -> Pr {
    let mut pr = titled_for_sort(number, "t");
    pr.author.login = login.to_string();
    pr
}

#[test]
fn the_author_sort_goes_by_login_ignoring_case_and_reverses() {
    let mut app = app_with(vec![
        by_author(1, "carol"),
        by_author(2, "app/renovate"),
        by_author(3, "Alice"),
        by_author(4, "bob"),
        by_author(5, "alice"),
    ]);
    press(&mut app, "ssss");
    assert_eq!(app.sort(), (Sort::Author, false));
    // `Alice` and `alice` together, in the order listed, then `app/renovate` as `al` is before `ap`.
    assert_eq!(numbers(&app), [3, 5, 2, 4, 1]);
    press(&mut app, "S");
    // The other way round, with the two `alice`s still in the order listed.
    assert_eq!(numbers(&app), [1, 4, 2, 3, 5]);
}

#[test]
fn the_author_sort_is_left_out_of_the_cycle_when_every_pr_has_the_same_author() {
    let mut app = App::new("PRs".to_string(), "is:pr author:alice".to_string(), false);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(vec![by_author(1, "alice")])));
    let mut seen = Vec::new();
    for _ in 0..5 {
        press(&mut app, "s");
        seen.push(app.sort().0);
    }
    assert_eq!(
        seen,
        [
            Sort::Number,
            Sort::Title,
            Sort::Review,
            Sort::Updated,
            Sort::Checks
        ]
    );
}

#[test]
fn a_sort_by_author_is_named_in_the_title_when_the_search_makes_its_column_go() {
    // Sorted by author while the column is there; then a search for one author takes it away.
    let mut app = app_with(vec![by_author(1, "bob"), by_author(2, "alice")]);
    press(&mut app, "ssss");
    assert_eq!(app.sort(), (Sort::Author, false));
    assert_eq!(app.title(), "PRs");
    press(&mut app, "/");
    key(&mut app, KeyCode::Backspace);
    press(&mut app, " author:alice");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(vec![by_author(2, "alice")])));
    assert_eq!(app.sort(), (Sort::Author, false));
    assert_eq!(app.title(), "PRs · sorted by author ▲");
}

fn titled_for_sort(number: u64, title: &str) -> Pr {
    pr_with(
        number,
        title,
        "OPEN",
        "",
        "MERGEABLE",
        serde_json::json!([{"__typename": "StatusContext", "context": "ci", "state": "SUCCESS"}]),
    )
}

#[test]
fn a_sort_is_named_in_the_title_only_when_its_column_is_hidden() {
    let mut app = app_to_sort();
    for _ in 0..5 {
        press(&mut app, "s");
        assert_eq!(app.title(), "PRs");
    }
    // Hiding the sorted column leaves the sort, and the title says what it is.
    app.toggle_column(Column::Updated);
    assert_eq!(app.title(), "PRs · sorted by updated ▲");
    press(&mut app, "S");
    assert_eq!(app.title(), "PRs · sorted by updated ▼");
    app.toggle_column(Column::Updated);
    assert_eq!(app.title(), "PRs");
    press(&mut app, "s");
    assert_eq!(app.title(), "PRs");
}

#[test]
fn each_sort_orders_the_open_prs_and_leaves_the_closed_ones_last() {
    let mut app = app_to_sort();
    press(&mut app, "s");
    assert_eq!(numbers(&app), [1, 2, 3, 4, 5, 6], "number");
    press(&mut app, "s");
    // Case is ignored.
    assert_eq!(numbers(&app), [1, 2, 3, 4, 5, 6], "title");
    press(&mut app, "s");
    // Those needing a review, in the order listed, then changes requested, then approved.
    assert_eq!(numbers(&app), [2, 1, 5, 4, 3, 6], "review");
    press(&mut app, "s");
    // They all have the same author, so the order they were listed in.
    assert_eq!(numbers(&app), [4, 2, 5, 1, 3, 6], "author");
    press(&mut app, "s");
    // The longest since an update first.
    assert_eq!(numbers(&app), [3, 2, 1, 5, 4, 6], "updated");
    press(&mut app, "s");
    assert_eq!(numbers(&app), [5, 1, 3, 2, 4, 6], "checks again");
}

#[test]
fn sorting_keeps_the_highlight_and_the_selection_on_their_prs() {
    let mut app = app_to_sort();
    // Highlight 3 and select it, and select 4 too.
    press(&mut app, "jj ");
    assert_eq!(app.highlighted().unwrap().pr.number, 3);
    press(&mut app, "jj ");
    press(&mut app, "kk");
    assert_eq!(app.selected, HashSet::from([3, 4]));
    assert_eq!(app.highlighted().unwrap().pr.number, 3);
    for _ in 0..5 {
        press(&mut app, "s");
        assert_eq!(app.highlighted().unwrap().pr.number, 3, "{}", app.title());
        assert_eq!(app.selected, HashSet::from([3, 4]));
        assert_eq!(app.visible.len(), 6);
    }
}

#[test]
fn sorting_with_a_filter_keeps_the_filter_and_the_highlight() {
    let mut app = app_to_sort();
    press(&mut app, "falpha");
    key(&mut app, KeyCode::Enter);
    assert_eq!(visible_numbers(&app), [1]);
    press(&mut app, "s");
    assert_eq!(visible_numbers(&app), [1]);
    assert_eq!(app.highlighted().unwrap().pr.number, 1);
    assert_eq!(app.filter.value(), "alpha");
}

#[test]
fn the_sort_stays_for_a_refresh_and_a_new_search_and_the_highlight_stays_on_its_pr() {
    let mut app = app_to_sort();
    press(&mut app, "ss");
    assert_eq!(app.sort(), (Sort::Title, false));
    // The highlight went with PR 5 through the sorts; move it to PR 2.
    key(&mut app, KeyCode::Home);
    press(&mut app, "j");
    assert_eq!(app.highlighted().unwrap().pr.number, 2);

    ctrl_r(&mut app);
    let (id, _) = app.list_to_fetch().unwrap();
    // The same PRs again, in GitHub's order.
    app.update(Msg::PrsLoaded(
        id,
        Ok(vec![
            sortable(5, "echo", "CHANGES_REQUESTED", "SUCCESS", "4"),
            sortable(3, "Charlie", "APPROVED", "SUCCESS", "1"),
            sortable(2, "Bravo", "", "PENDING", "2"),
            sortable(1, "alpha", "", "SUCCESS", "3"),
        ]),
    ));
    assert_eq!(numbers(&app), [1, 2, 3, 5]);
    assert_eq!(app.highlighted().unwrap().pr.number, 2);

    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(
        id,
        Ok(vec![
            sortable(9, "zulu", "", "SUCCESS", "1"),
            sortable(8, "Yankee", "", "SUCCESS", "1"),
        ]),
    ));
    assert_eq!(numbers(&app), [8, 9]);
    assert_eq!(app.sort(), (Sort::Title, false));
}

#[test]
fn prs_that_tie_stay_in_the_order_github_listed_however_often_the_list_is_sorted() {
    let mut app = app_with(vec![
        sortable(3, "same", "", "SUCCESS", "1"),
        sortable(1, "same", "", "SUCCESS", "1"),
        sortable(2, "same", "", "SUCCESS", "1"),
    ]);
    for _ in 0..7 {
        press(&mut app, "s");
        // Number puts them in order; the other sorts have nothing to say and leave GitHub's.
        let expected: &[u64] = if app.sort().0 == Sort::Number {
            &[1, 2, 3]
        } else {
            &[3, 1, 2]
        };
        assert_eq!(numbers(&app), expected, "{}", app.title());
    }
}

#[test]
fn a_row_refresh_keeps_the_place_it_was_listed_at() {
    let mut app = app_to_sort();
    press(&mut app, "s");
    let refreshed = sortable(4, "delta", "", "SUCCESS", "5");
    app.update(Msg::PrRefreshed(4, Ok(refreshed)));
    // The refreshed PR stays where it was until the list is sorted again.
    assert_eq!(numbers(&app), [1, 2, 3, 4, 5, 6]);
    press(&mut app, "sssss");
    // Passing now, so it goes with the passing ones, in the order listed.
    assert_eq!(numbers(&app), [4, 5, 1, 3, 2, 6]);
}

#[test]
fn all_three_boxes_take_the_word_editing_keys() {
    let alt = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::ALT));
    let mut app = app_with(vec![pr_with(
        1,
        "Bump alpha",
        "OPEN",
        "",
        "MERGEABLE",
        serde_json::json!([]),
    )]);
    // The search box: Alt-Backspace deletes the last word.
    press(&mut app, "/");
    app.query = Input::new("is:pr label:deps".to_string());
    app.update(alt(KeyCode::Backspace));
    assert_eq!(app.query.value(), "is:pr ");
    app.update(Msg::Key(KeyCode::Esc.into()));
    // The filter box: Alt-B then Alt-D deletes the word the cursor moved to.
    press(&mut app, "fone two");
    app.update(alt(KeyCode::Char('b')));
    app.update(alt(KeyCode::Char('d')));
    assert_eq!(app.filter.value(), "one ");
    app.update(Msg::Key(KeyCode::Esc.into()));
    // The columns box.
    press(&mut app, "c");
    app.columns_input = Input::new("(?<a>x) beta".to_string());
    app.update(alt(KeyCode::Backspace));
    assert_eq!(app.columns_input.value(), "(?<a>x) ");
}

#[test]
fn ctrl_enter_reviews_the_selection_like_ctrl_g() {
    let open = |app: &mut App| matches!(app.mode, Mode::Confirm(_));
    let ctrl_enter = Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
    let mut with_g = app_with(vec![pr_with(
        1,
        "Bump",
        "OPEN",
        "",
        "MERGEABLE",
        serde_json::json!([]),
    )]);
    let mut with_enter = app_with(vec![pr_with(
        1,
        "Bump",
        "OPEN",
        "",
        "MERGEABLE",
        serde_json::json!([]),
    )]);
    for app in [&mut with_g, &mut with_enter] {
        press(app, " ");
    }
    ctrl_g(&mut with_g);
    with_enter.update(ctrl_enter);
    assert!(open(&mut with_g) && open(&mut with_enter));
    let numbers = |app: &App| match &app.mode {
        Mode::Confirm(confirm) => confirm.prs.iter().map(|pr| pr.number).collect::<Vec<_>>(),
        _ => unreachable!(),
    };
    assert_eq!(numbers(&with_g), numbers(&with_enter));
    // A plain Enter still reviews only the highlighted PR through its own path, and is not this.
    let mut plain = app_with(vec![pr_with(
        1,
        "Bump",
        "OPEN",
        "",
        "MERGEABLE",
        serde_json::json!([]),
    )]);
    plain.update(Msg::Key(KeyCode::Enter.into()));
    assert!(matches!(plain.mode, Mode::Confirm(_)));
}
