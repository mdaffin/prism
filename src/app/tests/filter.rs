use super::*;

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
