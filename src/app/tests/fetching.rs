use super::*;

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
fn cached_diffs_are_capped_to_those_nearest_the_highlight() {
    use super::work::MAX_CACHED_DIFFS;
    let count = MAX_CACHED_DIFFS as u64 * 2;
    let mut app = app_with_rows(count);
    // Load every diff the prefetch asks for, until it asks for no more.
    let load_all = |app: &mut App| {
        loop {
            let fetches = app.diffs_to_fetch();
            if fetches.is_empty() {
                break;
            }
            for (number, _) in fetches {
                app.diff_loaded(number, Ok(String::new()));
            }
        }
    };
    load_all(&mut app);
    // Only the first rows, the ones nearest the highlight, were fetched.
    assert_eq!(app.diffs.len(), MAX_CACHED_DIFFS);
    assert!(app.diffs.contains_key(&1) && !app.diffs.contains_key(&(count)));

    // Moving to the middle drops those above it, and doesn't fetch them again.
    app.table.select(Some(MAX_CACHED_DIFFS));
    load_all(&mut app);
    assert_eq!(app.diffs.len(), MAX_CACHED_DIFFS);
    assert!(
        app.diffs.contains_key(&(MAX_CACHED_DIFFS as u64 + 1)) && app.diffs.contains_key(&count)
    );
    assert!(!app.diffs.contains_key(&1) && !app.diffs.contains_key(&2));
    assert!(app.diffs_to_fetch().is_empty());
}

#[test]
fn a_new_search_stops_waiting_for_fetches_of_pr_it_no_longer_lists() {
    let mut app = app_with_rows(3);
    let details = app.details_to_fetch();
    let diffs = app.diffs_to_fetch();
    assert_eq!(details.len(), 3);

    // The new list drops #1 and #3 but keeps #2, which was still loading.
    search_and_load(&mut app, prs([2, 4]));
    assert!(!app.details.contains_key(&1) && !app.details.contains_key(&3));
    assert!(!app.diffs.contains_key(&1) && !app.diffs.contains_key(&3));
    assert!(matches!(app.details[&2], Fetch::Loading(_)));
    assert!(matches!(app.diffs[&2], Fetch::Loading(_)));

    // Their slots are free for the new list's PRs, and the late results are ignored.
    assert_eq!(started(app.details_to_fetch()), [4]);
    for &(number, id) in &details {
        app.update(Msg::DetailsLoaded(
            number,
            id,
            Ok(pr_details("old", serde_json::json!([]))),
        ));
    }
    for &(number, id) in &diffs {
        app.update(Msg::DiffLoaded(number, id, Diff::parse("old")));
    }
    assert!(!app.details.contains_key(&1) && !app.diffs.contains_key(&3));
    assert!(loaded_body(&app, 2).is_some());
    assert!(matches!(app.diffs[&2], Fetch::Loaded(_)));
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

    search_and_load(&mut app, prs(1..=1));
    assert_eq!(app.title(), "PRs");
}

#[test]
fn ctrl_r_fetches_the_submitted_query() {
    let mut app = app_with_rows(3);
    ctrl(&mut app, 'r');
    let (_, query) = app.list_to_fetch().unwrap();
    assert_eq!(query, "is:pr");
}

#[test]
fn ctrl_r_is_ignored_while_a_search_runs_and_under_dialogs() {
    let mut app = app_with_rows(3);
    ctrl(&mut app, 'r');
    assert!(app.list_to_fetch().is_some());
    // The fetch is running.
    ctrl(&mut app, 'r');
    assert_eq!(app.list_to_fetch(), None);

    let mut app = app_with_rows(3);
    press(&mut app, "?");
    ctrl(&mut app, 'r');
    assert_eq!(app.list_to_fetch(), None);
    key(&mut app, KeyCode::Esc);
    press(&mut app, " ");
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Confirm(_)));
    ctrl(&mut app, 'r');
    assert_eq!(app.list_to_fetch(), None);
}

#[test]
fn ctrl_r_keeps_the_selection_by_number_when_rows_move() {
    let mut app = app_with_rows(3);
    select_rows(&mut app, &[0, 2]);
    assert_eq!(app.selected, HashSet::from([1, 3]));
    ctrl(&mut app, 'r');
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
    search_and_load(&mut app, prs([1, 2, 3]));
    assert!(app.selected.is_empty());
}

#[test]
fn ctrl_r_keeps_the_highlight_on_the_same_pr_and_clears_the_filter() {
    let mut app = app_with_titles();
    press(&mut app, "jfbeta");
    key(&mut app, KeyCode::Enter);
    ctrl(&mut app, 'r');
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(
        id,
        Ok(vec![titled(3, "c"), titled(2, "Bump beta"), titled(1, "a")]),
    ));
    assert_eq!(app.filter.value(), "");
    assert_eq!(visible_numbers(&app), [3, 2, 1]);
    assert_eq!(app.highlighted().unwrap().pr.number, 2);

    // If it is gone, the first row is highlighted.
    refresh_and_load(&mut app, Ok(vec![titled(8, "x"), titled(9, "y")]));
    assert_eq!(app.highlighted().unwrap().pr.number, 8);
}

#[test]
fn ctrl_r_drops_cached_details_and_diffs_but_a_new_search_keeps_them() {
    let mut app = app_with_rows(2);
    app.details_to_fetch();
    app.diffs_to_fetch();
    assert_eq!(app.details.len(), 2);

    search_and_load(&mut app, prs(1..=2));
    assert_eq!((app.details.len(), app.diffs.len()), (2, 2));

    refresh_and_load(&mut app, Ok(prs(1..=2)));
    assert_eq!((app.details.len(), app.diffs.len()), (0, 0));
    // So they are fetched again.
    assert_eq!(started(app.details_to_fetch()), [1, 2]);
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

    refresh_and_load(&mut app, Ok(prs(1..=2)));
    assert!(app.details.is_empty() && app.diffs.is_empty());

    // The fetches that were running now finish, with what was current before the refresh.
    for &(number, id) in &details {
        app.update(Msg::DetailsLoaded(
            number,
            id,
            Ok(pr_details("old", serde_json::json!([]))),
        ));
    }
    for &(number, id) in &diffs {
        app.update(Msg::DiffLoaded(number, id, Diff::parse("old")));
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

    refresh_and_load(&mut app, Ok(prs(1..=1)));
    let after = app.details_to_fetch()[0].1;
    assert_ne!(before, after);

    // The old fetch finishes first and is ignored, so the PR is still loading.
    app.update(Msg::DetailsLoaded(
        1,
        before,
        Ok(pr_details("old", serde_json::json!([]))),
    ));
    assert!(matches!(app.details[&1], Fetch::Loading(current) if current == after));
    app.update(Msg::DetailsLoaded(
        1,
        after,
        Ok(pr_details("new", serde_json::json!([]))),
    ));
    assert_eq!(loaded_body(&app, 1).as_deref(), Some("new"));
    // A late duplicate doesn't overwrite what loaded.
    app.update(Msg::DetailsLoaded(
        1,
        before,
        Ok(pr_details("old", serde_json::json!([]))),
    ));
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
        app.update(Msg::DetailsLoaded(
            number,
            id,
            Ok(pr_details("d", serde_json::json!([]))),
        ));
    }
    assert!(matches!(app.details[&1], Fetch::Loading(_)));
    assert_eq!(loaded_body(&app, 2).as_deref(), Some("d"));
    for &(number, id) in &diffs {
        app.update(Msg::DiffLoaded(number, id, Diff::parse("")));
    }
    assert!(!app.diffs.contains_key(&1));
    assert!(matches!(app.diffs[&2], Fetch::Loaded(_)));
}

#[test]
fn a_failed_ctrl_r_keeps_the_list_and_the_selection() {
    let mut app = app_with_rows(3);
    press(&mut app, " ");
    refresh_and_load(&mut app, Err(color_eyre::eyre::eyre!("boom")));
    assert_eq!(numbers(&app), [1, 2, 3]);
    assert_eq!(app.selected, HashSet::from([1]));
    assert!(app.errors.text().unwrap().contains("boom"));
}
