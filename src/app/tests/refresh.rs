use super::*;

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
    key(&mut app, KeyCode::Enter);
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
    ctrl(&mut app, 'r');
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
    ctrl(&mut app, 'r');
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
