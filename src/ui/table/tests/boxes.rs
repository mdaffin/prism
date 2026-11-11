//! The tests of the search, filter and columns boxes and of the suggestions under the search
//! box.

use super::*;

#[test]
fn a_fuzzy_filter_shows_the_rows_it_matches() {
    let mut app = app_with(vec![
        by("alice", 1, "Bump alpha"),
        by("app/renovate", 2, "Bump beta"),
        by("bob", 3, "Update gamma"),
    ]);
    press(&mut app, "frnv");
    let screen = lines(&draw(&mut app, 200, 30));

    assert!(shows(&screen, "Bump beta"));
    assert!(!shows(&screen, "Bump alpha"));
    assert!(!shows(&screen, "Update gamma"));
}

#[test]
fn draws_the_filter_box_and_the_filtered_rows() {
    let mut app = app_with(vec![
        pr(1, "Bump alpha", "OPEN", ""),
        pr(2, "Bump beta", "OPEN", ""),
        pr(3, "Update gamma", "OPEN", ""),
    ]);
    // No box until there is a filter.
    let screen = lines(&draw(&mut app, 200, 30));
    assert!(!shows(&screen, "Filter"));

    press(&mut app, " jjfgamma");
    let screen = lines(&draw(&mut app, 200, 30));
    assert!(shows(&screen, "Filter"));
    assert!(shows(&screen, "Update gamma"));
    assert!(!shows(&screen, "Bump alpha"));
    // PR 1 is selected, and the filter hides it.
    assert!(screen.last().unwrap().contains("1 selected (1 hidden)"));
}

#[test]
fn shows_a_column_for_each_group_of_the_column_regex() {
    let mut app = app_with(vec![
        pr(1, "Bump alpha from 1.0 to 1.2", "OPEN", ""),
        pr(2, "Fix beta", "OPEN", ""),
    ]);
    app.columns = Some(
        model::parse_column_regex(r"Bump (?<container>.+?)(?: from \S+)? to (?<version>\S+)$")
            .unwrap(),
    );
    let screen = lines(&draw(&mut app, 200, 30));

    let header = table_header_line(&app, &screen);
    assert!(header.contains("container") && header.contains("version"));
    assert!(!header.contains("Title"));
    let bump = table_row_line(&app, &screen, "#1");
    assert!(bump.contains("alpha") && bump.contains("1.2"));
    assert!(!bump.contains("Bump"));
    // A title that doesn't match gets empty cells; the title is still not shown.
    let fix = table_row_line(&app, &screen, "#2");
    assert!(!fix.contains("Fix beta"));
}

/// Clears the columns box and types `regex` into it.
fn type_columns(app: &mut App, regex: &str) {
    for _ in 0..app.columns_input.value().chars().count() {
        key(app, KeyCode::Backspace);
    }
    press(app, regex);
}

const CONTAINER_VERSION: &str = r"Bump (?<container>.+?) to (?<version>\S+)$";

#[test]
fn c_opens_the_columns_box() {
    let mut app = app_with(vec![pr(1, "Bump alpha to 1.2", "OPEN", "")]);
    press(&mut app, "c");
    assert!(app.typing == Typing::Columns);
    let screen = lines(&draw(&mut app, 200, 30));
    assert!(shows(&screen, "Columns"));
    assert!(app.drawn.areas.columns != Rect::default());
}

#[test]
fn a_valid_regex_in_the_columns_box_changes_the_headers_on_enter() {
    let mut app = app_with(vec![pr(1, "Bump alpha to 1.2", "OPEN", "")]);
    let screen = lines(&draw(&mut app, 200, 30));
    assert!(shows(&screen, "Title"));

    press(&mut app, "c");
    press(&mut app, CONTAINER_VERSION);
    key(&mut app, KeyCode::Enter);
    assert!(app.typing == Typing::Nothing);
    let screen = lines(&draw(&mut app, 200, 30));
    let header = table_header_line(&app, &screen);
    assert!(header.contains("container") && header.contains("version"));
    assert!(!header.contains("Title"));
    // The box is gone, and has no area to catch a click.
    assert_eq!(app.drawn.areas.columns, Rect::default());
}

#[test]
fn reopening_the_columns_box_starts_from_the_regex_in_use() {
    let mut app = app_with(vec![pr(1, "Bump alpha to 1.2", "OPEN", "")]);
    app.columns = Some(model::parse_column_regex(CONTAINER_VERSION).unwrap());
    press(&mut app, "c");
    assert_eq!(app.columns_input.value(), CONTAINER_VERSION);
}

#[test]
fn an_empty_regex_in_the_columns_box_goes_back_to_the_title() {
    let mut app = app_with(vec![pr(1, "Bump alpha to 1.2", "OPEN", "")]);
    app.columns = Some(model::parse_column_regex(CONTAINER_VERSION).unwrap());
    press(&mut app, "c");
    type_columns(&mut app, "");
    key(&mut app, KeyCode::Enter);
    assert!(app.columns.is_none());
    let screen = lines(&draw(&mut app, 200, 30));
    assert!(shows(&screen, "Title"));
}

#[test]
fn an_invalid_regex_shows_its_error_and_keeps_the_old_one() {
    let mut app = app_with(vec![pr(1, "Bump alpha to 1.2", "OPEN", "")]);
    app.columns = Some(model::parse_column_regex(r"Bump (?<container>\S+)").unwrap());

    for (bad, reason) in [
        ("(", "unclosed group"),
        ("no groups", "at least one named group"),
        ("(?<title>x)", "clash"),
    ] {
        press(&mut app, "c");
        type_columns(&mut app, bad);
        key(&mut app, KeyCode::Enter);
        // The box stays open, with the reason in its title.
        assert!(app.typing == Typing::Columns, "{bad}");
        let screen = lines(&draw(&mut app, 200, 30));
        let title = screen.iter().find(|line| line.contains("Columns")).unwrap();
        assert!(title.contains(reason), "{bad}: {title}");
        let header = table_header_line(&app, &screen);
        assert!(header.contains("container"), "{bad}: the old regex stays");

        // Editing clears the error, and Esc leaves the old regex in use.
        press(&mut app, "x");
        assert!(app.columns_error.is_none());
        key(&mut app, KeyCode::Esc);
        assert!(app.typing == Typing::Nothing);
        assert_eq!(
            app.columns.as_ref().unwrap().as_str(),
            r"Bump (?<container>\S+)"
        );
    }
}

#[test]
fn the_filter_follows_the_columns_when_they_change() {
    let mut app = app_with(vec![
        pr(1, "Bump alpha to 1.2", "OPEN", ""),
        pr(2, "Bump beta to 3.4", "OPEN", ""),
    ]);
    press(&mut app, "fBump");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.visible.len(), 2);

    // "Bump" is in the titles but not in any column.
    press(&mut app, "c");
    press(&mut app, r"Bump (?<container>.+?) to (?<version>\S+)$");
    key(&mut app, KeyCode::Enter);
    assert!(app.visible.is_empty());

    // Back to the title, and it matches again.
    press(&mut app, "c");
    type_columns(&mut app, "");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.visible.len(), 2);
}

#[test]
fn a_click_on_the_columns_box_moves_its_cursor_and_elsewhere_drops_the_edit() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    press(&mut app, "c");
    press(&mut app, "(?<a>x)");
    draw_wide(&mut app);
    let area = app.drawn.areas.columns;
    press_at(&mut app, Position::new(area.x + 1 + 2, area.y + 1));
    assert!(app.typing == Typing::Columns);
    assert_eq!(app.columns_input.cursor(), 2);

    // A click on the table leaves the box; the regex was never applied.
    press_at(&mut app, Position::new(2, area.y + 6));
    assert!(app.typing == Typing::Nothing);
    assert!(app.columns.is_none());
}

#[test]
fn the_filter_matches_regex_group_cells_not_the_raw_title() {
    let mut app = app_with(vec![
        pr(1, "Bump alpha to 1.2", "OPEN", ""),
        pr(2, "Bump beta to 3.4", "OPEN", ""),
    ]);
    app.columns =
        Some(model::parse_column_regex(r"Bump (?<container>.+?) to (?<version>\S+)$").unwrap());

    press(&mut app, "f3.4");
    let screen = lines(&draw(&mut app, 200, 30));
    assert!(shows(&screen, "#2"));
    assert!(!shows(&screen, "#1"));

    // "Bump" is in the titles but not in any column.
    for _ in 0..3 {
        key(&mut app, KeyCode::Backspace);
    }
    press(&mut app, "Bump");
    let screen = lines(&draw(&mut app, 200, 30));
    assert!(shows(&screen, "No PRs match"));
}

#[test]
fn the_search_and_filter_boxes_colour_only_their_border_and_title_while_edited() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    press(&mut app, "fbump");
    key(&mut app, KeyCode::Enter);
    let buffer = draw_wide(&mut app);
    let search = app.drawn.areas.search;
    let filter = app.drawn.areas.filter;
    // Not being edited: nothing is cyan.
    assert_eq!(buffer[(search.x, search.y + 1)].fg, Color::Reset);
    assert_eq!(buffer[(filter.x, filter.y + 1)].fg, Color::Reset);

    press(&mut app, "/");
    let buffer = draw_wide(&mut app);
    assert_eq!(buffer[(search.x, search.y + 1)].fg, Color::Cyan);
    assert_eq!(buffer[(search.x + 1, search.y)].fg, Color::Cyan);
    assert_eq!(buffer[(search.x + 1, search.y + 1)].fg, Color::Reset);
    assert_eq!(buffer[(filter.x, filter.y + 1)].fg, Color::Reset);

    key(&mut app, KeyCode::Esc);
    press(&mut app, "f");
    let buffer = draw_wide(&mut app);
    assert_eq!(buffer[(filter.x, filter.y + 1)].fg, Color::Cyan);
    assert_eq!(buffer[(filter.x + 1, filter.y + 1)].fg, Color::Reset);
    assert_eq!(buffer[(search.x, search.y + 1)].fg, Color::Reset);
}

#[test]
fn the_suggestions_are_listed_under_the_word_with_the_chosen_one_reversed() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    press(&mut app, "/ la");
    let buffer = draw_wide(&mut app);
    let search = app.drawn.areas.search;
    let label = find(&buffer, " label ").unwrap();
    // Inside the box's border, which starts under the word: "is:pr " is six columns.
    assert_eq!(label.y, search.bottom() + 1);
    assert_eq!(label.x, search.x + 1 + 6 + 1);
    let language = find(&buffer, " language ").unwrap();
    assert_eq!(language.y, label.y + 1);
    // Nothing is chosen to begin with, and the hints say Enter searches.
    assert!(!reversed(&buffer, label));
    assert!(!reversed(&buffer, language));
    let status = lines(&buffer).pop().unwrap();
    assert!(status.contains("Enter: Search | Tab/Up/Down: Choose"));

    key(&mut app, KeyCode::Tab);
    let buffer = draw_wide(&mut app);
    assert!(reversed(&buffer, label));
    assert!(!reversed(&buffer, language));
    // Now the hints say Enter takes it.
    assert!(lines(&buffer).pop().unwrap().contains("Enter: Take"));

    key(&mut app, KeyCode::Tab);
    let buffer = draw_wide(&mut app);
    assert!(!reversed(&buffer, label));
    assert!(reversed(&buffer, language));
}

#[test]
fn many_suggestions_scroll_to_keep_the_highlight_in_view() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    press(&mut app, "/ sort:");
    for _ in 0..9 {
        key(&mut app, KeyCode::Down);
    }
    let buffer = draw_wide(&mut app);
    let screen = lines(&buffer);
    // Ten values, eight rows: the ninth is highlighted at the bottom and the first has gone.
    assert!(find(&buffer, " interactions-desc ").is_some());
    assert!(!shows(&screen, " created-desc "));
}

#[test]
fn the_suggestions_are_left_out_where_there_is_no_room_for_a_row() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    press(&mut app, "/ la");
    // Too short for the search box to be drawn whole, and just tall enough for it alone.
    for height in [6, 7] {
        let buffer = draw(&mut app, 200, height);
        assert!(find(&buffer, " label ").is_none());
    }
}

/// Clicks `column` characters into the text of the search box, as last drawn.
fn click_search(app: &mut App, column: u16) {
    let area = app.drawn.areas.search;
    press_at(app, Position::new(area.x + 1 + column, area.y + 1));
}

fn click_filter(app: &mut App, column: u16) {
    let area = app.drawn.areas.filter;
    press_at(app, Position::new(area.x + 1 + column, area.y + 1));
}

#[test]
fn a_click_on_the_search_box_edits_the_query_with_the_cursor_there() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    draw_wide(&mut app);
    click_search(&mut app, 3);
    assert!(app.typing == Typing::Search);
    assert_eq!(app.query.cursor(), 3);
    // Past the end of the text puts the cursor at the end.
    click_search(&mut app, 60);
    assert_eq!(app.query.cursor(), app.query.value().chars().count());
}

#[test]
fn a_click_on_the_filter_box_edits_the_filter_only_while_it_is_drawn() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    draw_wide(&mut app);
    assert_eq!(app.drawn.areas.filter, Rect::default());
    press_at(&mut app, Position::new(2, 4));
    assert!(app.typing != Typing::Filter);

    press(&mut app, "fbump");
    key(&mut app, KeyCode::Enter);
    draw_wide(&mut app);
    assert!(app.typing == Typing::Nothing);
    click_filter(&mut app, 2);
    assert!(app.typing == Typing::Filter);
    assert_eq!(app.filter.cursor(), 2);

    // Once the filter is emptied it is not drawn, so it has no area.
    assert!(app.drawn.areas.filter != Rect::default());
    key(&mut app, KeyCode::Esc);
    draw_wide(&mut app);
    assert_eq!(app.drawn.areas.filter, Rect::default());
}

#[test]
fn a_click_on_the_other_box_switches_to_it() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    press(&mut app, "fbump");
    draw_wide(&mut app);
    click_search(&mut app, 0);
    assert!(app.typing == Typing::Search);
    assert_eq!(app.filter.value(), "bump");
    click_filter(&mut app, 0);
    assert!(app.typing == Typing::Filter);
}

#[test]
fn a_click_on_the_table_leaves_the_search_box_keeping_its_text_unsearched() {
    let mut app = app_with(vec![
        pr(1, "Bump alpha", "OPEN", ""),
        pr(2, "Bump beta", "OPEN", ""),
    ]);
    draw_wide(&mut app);
    press(&mut app, "/");
    press(&mut app, " label:deps");
    click_table_row(&mut app, 1, 20);

    assert!(app.typing == Typing::Nothing);
    assert_eq!(app.query.value(), "is:pr label:deps");
    assert!(app.list_to_fetch().is_none(), "not submitted");
    // The click did what it does anywhere else.
    assert_eq!(app.highlighted().unwrap().pr.number, 2);
    let screen = lines(&draw_wide(&mut app));
    assert!(shows(&screen, "not searched"));

    // `/` resumes the same text, and Esc restores the searched query.
    press(&mut app, "/");
    assert_eq!(app.query.value(), "is:pr label:deps");
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.query.value(), "is:pr");
    let screen = lines(&draw_wide(&mut app));
    assert!(!shows(&screen, "not searched"));
}

#[test]
fn a_click_on_a_pane_leaves_the_filter_box_keeping_its_filter() {
    let mut app = app_with(vec![
        pr(1, "Bump alpha", "OPEN", ""),
        pr(2, "Update beta", "OPEN", ""),
    ]);
    press(&mut app, "fbump");
    draw_wide(&mut app);
    let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
    press_at(&mut app, diff);

    assert!(app.typing == Typing::Nothing);
    assert_eq!(app.focus, Pane::Diff);
    assert_eq!(app.filter.value(), "bump");
    assert_eq!(app.visible_rows().count(), 1);
}

#[test]
fn ctrl_r_runs_the_submitted_query_not_the_unsearched_text() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    draw_wide(&mut app);
    press(&mut app, "/");
    press(&mut app, " label:deps");
    // A click in the Comments pane, away from the table and the search box.
    let comments = app.drawn.areas.comments;
    press_at(&mut app, Position::new(comments.x + 2, comments.y + 1));
    ctrl(&mut app, 'r');
    let (_, query) = app.list_to_fetch().unwrap();
    assert_eq!(query, "is:pr");
}

#[test]
fn says_when_the_filter_hides_every_pr_and_how_to_clear_it() {
    let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
    press(&mut app, "fzzz");
    for width in [200, 100] {
        let screen = lines(&draw(&mut app, width, 30)).join("\n");
        assert!(
            screen.contains("No PRs match the filter `zzz`. Press Esc to clear it."),
            "{width}: {screen}"
        );
    }
    // Esc does what it says.
    key(&mut app, KeyCode::Esc);
    assert!(app.visible.len() == 1);
}

#[test]
fn says_which_query_found_nothing_and_points_to_the_search_box() {
    for width in [200, 100] {
        let mut app = app_with_query("is:pr label:nothing", Vec::new());
        let screen = lines(&draw(&mut app, width, 30)).join("\n");
        assert!(
            screen
                .contains("No PRs found for `is:pr label:nothing`. Press / to change the search."),
            "{width}: {screen}"
        );
    }
}

#[test]
fn a_failed_first_search_points_to_trying_again() {
    let mut app = App::new("PRs".to_string(), "is:pr".to_string(), false);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Err(gh_failure())));
    let screen = lines(&draw(&mut app, 200, 30)).join("\n");
    assert!(
        screen.contains("No PRs loaded for `is:pr`. Press Ctrl-R"),
        "{screen}"
    );
}
