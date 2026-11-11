use super::*;

#[test]
fn enter_confirms_the_selected_prs_not_the_highlighted_one() {
    let mut app = app_with_rows(3);
    select_rows(&mut app, &[0, 2]);
    press(&mut app, "j");
    key(&mut app, KeyCode::Enter);
    assert_eq!(confirm_numbers(&app), [1, 3]);
    assert_eq!(confirm(&app).choices().len(), 3);
}

#[test]
fn closed_and_merged_prs_are_left_out_of_a_review_and_listed() {
    let prs = vec![pr(1, "OPEN"), pr(2, "CLOSED"), pr(3, "MERGED")];
    let mut app = app_with(prs);
    press(&mut app, " j j ");
    review(&mut app);
    assert_eq!(confirm_numbers(&app), [1]);
    assert_eq!(confirm(&app).left_out, [2, 3]);
}

#[test]
fn with_only_a_closed_pr_to_review_there_is_nothing_to_choose_but_cancel() {
    let prs = vec![pr(1, "OPEN"), pr(2, "CLOSED"), pr(3, "MERGED")];
    let mut app = app_with(prs);
    press(&mut app, "j ");
    key(&mut app, KeyCode::Enter);
    assert!(confirm(&app).prs.is_empty());
    assert_eq!(confirm(&app).choices(), [Choice::Cancel]);
}

#[test]
fn the_dialog_moves_between_choices_and_closes() {
    let mut app = app_with_rows(3);
    press(&mut app, " ");
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Right);
    assert_eq!(confirm(&app).choice, 1);
    for _ in 0..5 {
        key(&mut app, KeyCode::Tab);
    }
    assert_eq!(confirm(&app).choice, 2);
    key(&mut app, KeyCode::Enter);
    assert_list_mode(&app);

    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Esc);
    assert_list_mode(&app);
    assert!(!app.should_quit);
}

#[test]
fn a_list_loading_under_the_dialog_keeps_it_open() {
    let mut app = app_with_rows(3);
    press(&mut app, "/");
    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    press(&mut app, " ");
    key(&mut app, KeyCode::Enter);
    app.update(Msg::PrsLoaded(id, Ok(prs([7]))));
    assert_eq!(confirm_numbers(&app), [1]);
}

#[test]
fn approvals_run_at_most_four_at_once() {
    let limit = MAX_APPROVALS as u64;
    let mut app = app_with_rows(limit + 2);
    start_approving(&mut app, limit as usize + 2);
    assert_eq!(app.approvals_to_run(), (1..=limit).collect::<Vec<_>>());
    assert!(app.approvals_to_run().is_empty());
    approve(&mut app, 2, Ok(()));
    assert_eq!(app.approvals_to_run(), [limit + 1]);
    approve(&mut app, 1, Ok(()));
    approve(&mut app, 3, Ok(()));
    assert_eq!(app.approvals_to_run(), [limit + 2]);
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
    let summary = results(&app);
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
    review(&mut app);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Running(_)));
    assert_eq!(app.approvals_to_run(), [1]);
    app.update(Msg::Approved(old, 1, Ok(())));
    let batch = running(&app);
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
    let summary = results(&app);
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
    let summary = results(&app);
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
    let summary = results(&app);
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
    assert_list_mode(&app);
    assert!(!app.should_quit);
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
    review(&mut app);
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
    let batch = running(&app);
    assert_eq!(batch.progress(), (1, 4));
    assert_eq!(app.merge_to_run(), Some((1, false)));
    assert_eq!(app.merge_to_run(), None);
    merge(&mut app, 1, Ok(()));
    assert_eq!(app.merge_to_run(), Some((2, true)));
    merge(&mut app, 2, Ok(()));
    assert_eq!(app.merge_to_run(), Some((3, false)));
    merge(&mut app, 3, Err(color_eyre::eyre::eyre!("not mergeable")));
    assert_eq!(app.merge_to_run(), None);

    let summary = results(&app);
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
    let summary = results(&app);
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
    let summary = results(&app);
    assert_eq!(summary.merged, Some((0, 0)));
}
