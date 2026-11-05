use super::*;

/// A PR with a title, a review decision, a check that passes or not, and when it was updated.
fn sortable(number: u64, title: &str, review: &str, check: &str, updated: &str) -> Pr {
    let rollup = match check {
        "" => serde_json::json!([]),
        state => serde_json::json!([status_context(state)]),
    };
    PrBuilder::new(number)
        .title(title)
        .review(review)
        .checks(rollup)
        .updated_at(&format!("2026-01-0{updated}T00:00:00Z"))
        .build()
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
    ctrl(&mut app, 'r');
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
        titled_with_a_passing_check(1, "Bump zeta to 1.0"),
        titled_with_a_passing_check(2, "Bump alpha to 3.0"),
        titled_with_a_passing_check(3, "Bump alpha to 2.0"),
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
    PrBuilder::new(number)
        .checks(serde_json::json!([status_context("SUCCESS")]))
        .author(login)
        .build()
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

/// `titled`, with a passing check.
fn titled_with_a_passing_check(number: u64, title: &str) -> Pr {
    PrBuilder::new(number)
        .title(title)
        .checks(serde_json::json!([status_context("SUCCESS")]))
        .build()
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

    ctrl(&mut app, 'r');
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
