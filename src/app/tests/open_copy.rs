use super::*;

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
    with_check_rows(&mut app, 3, 0, 5);
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
    let details = pr_details(
        "",
        serde_json::json!([
            {"createdAt": "2026-01-02T03:00:00Z", "body": "a",
             "url": "https://example.test/pull/1#issuecomment-1"},
            {"createdAt": "2026-01-02T03:00:00Z", "body": "b"},
        ]),
    );
    app.details_loaded(1, Ok(details));
    with_comment_rows(&mut app, &[1, 1]);
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
    let details = pr_details(
        "",
        serde_json::json!([
            {"createdAt": "2026-01-02T03:00:00Z", "body": "approved",
             "url": "https://example.test/pull/1#pullrequestreview-1",
             "kind": {"Review": "APPROVED"}},
            {"createdAt": "2026-01-02T04:00:00Z", "body": "first",
             "url": "https://example.test/pull/1#discussion_r1",
             "kind": {"Thread": {"path": "a.rs", "line": 3, "resolved": false, "outdated": false}},
             "replies": [{"createdAt": "2026-01-02T05:00:00Z", "body": "reply"}]},
        ]),
    );
    app.details_loaded(1, Ok(details));
    with_comment_rows(&mut app, &[1, 1]);
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
    with_comment_rows(&mut app, &[1, 1]);
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
    let details = pr_details(
        "",
        serde_json::json!([
            {"createdAt": "2026-01-02T03:00:00Z", "body": "a",
             "url": "https://example.test/pull/1#issuecomment-1"},
            {"createdAt": "2026-01-02T03:00:00Z", "body": "b"},
        ]),
    );
    app.details_loaded(1, Ok(details));
    with_comment_rows(&mut app, &[1, 1]);
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
