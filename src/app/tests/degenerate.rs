//! A list with nothing to act on, a first load that fails, and keys in the wrong mode.

use super::*;

/// Every key that acts on the list or the highlighted row.
fn press_every_list_key(app: &mut App) {
    press(app, "jk");
    for code in [KeyCode::Home, KeyCode::End, KeyCode::PageDown] {
        key(app, code);
    }
    press(app, " osyS");
    key(app, KeyCode::Enter);
    ctrl(app, 'a');
    ctrl(app, 'o');
}

fn assert_nothing_to_act_on(app: &mut App) {
    assert_list_mode(app);
    assert!(app.selected.is_empty());
    assert!(app.highlighted().is_none());
    assert!(app.opens_to_run().is_empty());
    assert!(app.approvals_to_run().is_empty());
}

#[test]
fn the_keys_of_an_empty_list_do_nothing() {
    let mut app = app_with(Vec::new());
    press_every_list_key(&mut app);
    assert_nothing_to_act_on(&mut app);
}

#[test]
fn the_keys_of_a_list_the_filter_hides_every_row_of_do_nothing() {
    let mut app = app_with_titles();
    press(&mut app, "fzzz");
    key(&mut app, KeyCode::Enter);
    assert!(visible_numbers(&app).is_empty());
    press_every_list_key(&mut app);
    assert_nothing_to_act_on(&mut app);

    // Clearing the filter brings the rows back, with the first highlighted. `S` reversed them.
    key(&mut app, KeyCode::Esc);
    assert_eq!(visible_numbers(&app), [3, 2, 1]);
    assert_eq!(app.highlighted().unwrap().pr.number, 3);
}

#[test]
fn a_first_load_that_fails_leaves_loading_and_the_search_usable() {
    let mut app = App::new("PRs".to_string(), "is:pr".to_string(), false);
    assert!(matches!(app.mode, Mode::Loading));
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Err(color_eyre::eyre::eyre!("boom"))));

    assert_list_mode(&app);
    assert_eq!(app.errors.search.as_deref(), Some("Search failed: boom"));
    assert!(app.rows.is_empty());
    assert!(!app.list.searching);

    press(&mut app, "/");
    assert!(app.typing == Typing::Search);
    key(&mut app, KeyCode::Esc);
    ctrl(&mut app, 'r');
    let (_, query) = app.list_to_fetch().unwrap();
    assert_eq!(query, "is:pr");
}

/// Presses Ctrl-C and says whether the app is quitting.
fn quits_on_ctrl_c(mut app: App) -> bool {
    ctrl(&mut app, 'c');
    app.should_quit
}

#[test]
fn ctrl_c_quits_from_the_list_and_every_box() {
    assert!(quits_on_ctrl_c(app_with_rows(2)));
    for keys in ["/", "f", "c"] {
        let mut app = app_with_rows(2);
        press(&mut app, keys);
        assert!(app.typing != Typing::Nothing, "{keys}");
        assert!(quits_on_ctrl_c(app), "{keys}");
    }
}

#[test]
fn ctrl_c_quits_from_every_dialog_and_does_not_just_cancel_the_batch() {
    let mut app = app_with_rows(2);
    press(&mut app, " ");
    review(&mut app);
    assert!(matches!(app.mode, Mode::Confirm(_)));
    assert!(quits_on_ctrl_c(app));

    let mut app = app_with_rows(2);
    start_approving(&mut app, 1);
    assert!(quits_on_ctrl_c(app));

    let mut app = app_with_rows(2);
    start_approving(&mut app, 1);
    approve(&mut app, 1, Ok(()));
    assert!(matches!(app.mode, Mode::Results(_)));
    assert!(quits_on_ctrl_c(app));

    let mut app = app_with_rows(2);
    press(&mut app, "?");
    assert!(matches!(app.mode, Mode::Help(_)));
    assert!(quits_on_ctrl_c(app));
}

#[test]
fn ctrl_c_while_running_leaves_the_batch_as_it_was() {
    let mut app = app_with_rows(2);
    start_approving(&mut app, 1);
    ctrl(&mut app, 'c');
    assert!(app.should_quit);
    assert!(matches!(app.mode, Mode::Running(_)));
}

/// The highlight, the selection, whether no box is edited, the sort and the hidden panes.
type ListState = (Option<usize>, Vec<u64>, bool, (Sort, bool), Vec<Pane>);

/// The state that the list's keys would change.
fn list_state(app: &App) -> ListState {
    let mut selected: Vec<u64> = app.selected.iter().copied().collect();
    selected.sort();
    (
        app.table.selected(),
        selected,
        app.typing == Typing::Nothing,
        app.sort(),
        app.hidden_panes.clone(),
    )
}

fn assert_dialog_ignores_the_lists_keys(mut app: App, name: &str) {
    let before = list_state(&app);
    press(&mut app, "jfsp 1v");
    assert_eq!(list_state(&app), before, "{name}");
}

#[test]
fn the_lists_keys_do_nothing_under_a_dialog() {
    let selected = || {
        let mut app = app_with_rows(3);
        press(&mut app, " j");
        app
    };

    let mut app = selected();
    review(&mut app);
    assert!(matches!(app.mode, Mode::Confirm(_)));
    assert_dialog_ignores_the_lists_keys(app, "confirm");

    let mut app = app_with_rows(7);
    for _ in 0..6 {
        press(&mut app, " j");
    }
    ctrl(&mut app, 'o');
    assert!(matches!(app.mode, Mode::ConfirmOpen(_)));
    assert_dialog_ignores_the_lists_keys(app, "confirm open");

    let mut app = selected();
    press(&mut app, "?");
    assert!(matches!(app.mode, Mode::Help(_)));
    assert_dialog_ignores_the_lists_keys(app, "help");

    let mut app = selected();
    start_approving(&mut app, 1);
    approve(&mut app, 1, Ok(()));
    approve(&mut app, 2, Ok(()));
    assert!(matches!(app.mode, Mode::Results(_)));
    assert_dialog_ignores_the_lists_keys(app, "results");
}
