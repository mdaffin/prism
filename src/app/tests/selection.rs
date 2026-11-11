use super::*;

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
    ctrl(&mut app, 'a');
    assert_eq!(app.selected, HashSet::from([1, 5]));
    // Nothing starts: it only selects.
    assert_list_mode(&app);
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
    ctrl(&mut app, 'a');
    assert_eq!(app.selected, HashSet::from([1, 3]));
}

const PANES: [Pane; 5] = [
    Pane::Table,
    Pane::Checks,
    Pane::Comments,
    Pane::Description,
    Pane::Diff,
];

#[test]
fn ctrl_o_and_enter_do_nothing_when_none_are_selected() {
    for pane in PANES {
        let mut app = app_with_rows(3);
        app.focus = pane;
        press(&mut app, "j");
        ctrl(&mut app, 'o');
        review(&mut app);
        assert!(app.opens_to_run().is_empty(), "{pane:?}");
        assert!(matches!(app.mode, Mode::List), "{pane:?}");
    }
}

#[test]
fn ctrl_o_opens_the_selection_from_every_pane() {
    for pane in PANES {
        let mut app = app_with_rows(3);
        select_rows(&mut app, &[0, 2]);
        app.focus = pane;
        ctrl(&mut app, 'o');
        assert_eq!(app.opens_to_run(), [1, 3], "{pane:?}");
    }
}

#[test]
fn enter_reviews_the_selection_from_every_pane() {
    for pane in PANES {
        let mut app = app_with_rows(3);
        select_rows(&mut app, &[0, 2]);
        app.focus = pane;
        review(&mut app);
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
        ctrl(&mut app, 'a');
        assert_eq!(app.selected, HashSet::from([1]), "{pane:?}");
        ctrl(&mut app, 'r');
        assert!(app.list_to_fetch().is_some(), "{pane:?}");
    }
}

#[test]
fn ctrl_o_opens_the_selected_prs_in_table_order() {
    let mut app = app_with_rows(4);
    // Selected out of order: 1, then 3, then 2.
    press(&mut app, " jj k ");
    ctrl(&mut app, 'o');
    assert_eq!(app.opens_to_run(), [1, 2, 3]);
}

#[test]
fn ctrl_o_asks_before_opening_more_than_five() {
    // Up to the limit, the PRs open without asking.
    let limit = MAX_OPEN_WITHOUT_ASKING as u64;
    let mut app = app_with_rows(limit);
    for _ in 0..limit {
        press(&mut app, " j");
    }
    ctrl(&mut app, 'o');
    assert_list_mode(&app);
    assert_eq!(app.opens_to_run(), (1..=limit).collect::<Vec<_>>());

    // One more asks.
    let count = limit + 1;
    let mut app = app_with_rows(count);
    for _ in 0..count {
        press(&mut app, " j");
    }
    ctrl(&mut app, 'o');
    assert!(app.opens_to_run().is_empty());
    assert!(matches!(app.mode, Mode::ConfirmOpen(_)));
    // Cancel leaves everything alone.
    press(&mut app, "l");
    key(&mut app, KeyCode::Enter);
    assert_list_mode(&app);
    assert!(app.opens_to_run().is_empty());

    ctrl(&mut app, 'o');
    key(&mut app, KeyCode::Enter);
    assert_list_mode(&app);
    assert_eq!(app.opens_to_run(), (1..=count).collect::<Vec<_>>());
}

#[test]
fn a_pr_that_fails_to_open_is_shown_with_the_error() {
    let mut app = app_with_rows(2);
    app.update(Msg::Opened(2, Err(color_eyre::eyre::eyre!("no browser"))));
    assert_eq!(
        app.errors.text().as_deref(),
        Some("Opening #2 failed: no browser")
    );
    // A PR that did open leaves the error shown as it was.
    app.update(Msg::Opened(1, Ok(())));
    assert_eq!(
        app.errors.text().as_deref(),
        Some("Opening #2 failed: no browser")
    );
}

#[test]
fn space_toggles_the_highlighted_pr_until_new_results() {
    let mut app = app_with_rows(3);
    select_rows(&mut app, &[0, 2]);
    assert_eq!(app.selected, HashSet::from([1, 3]));
    press(&mut app, "k ");
    assert_eq!(app.selected, HashSet::from([1, 2, 3]));
    press(&mut app, " ");
    assert_eq!(app.selected, HashSet::from([1, 3]));

    // Not cleared until the new results arrive.
    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    assert_eq!(app.selected.len(), 2);
    app.update(Msg::PrsLoaded(id, Ok(prs([1, 2, 3]))));
    assert!(app.selected.is_empty());
}
