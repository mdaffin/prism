use super::*;

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
