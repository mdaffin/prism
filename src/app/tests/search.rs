use super::*;

#[test]
fn fetches_the_initial_query_once() {
    let mut app = App::new(String::new(), "is:pr state:open".to_string(), false);
    assert_eq!(
        app.list_to_fetch(),
        Some((1, "is:pr state:open".to_string()))
    );
    assert_eq!(app.list_to_fetch(), None);
}

#[test]
fn esc_in_the_list_clears_the_selection_and_never_quits() {
    let mut app = app_with_rows(3);
    press(&mut app, " j ");
    assert_eq!(app.selected.len(), 2);
    key(&mut app, KeyCode::Esc);
    assert!(app.selected.is_empty());
    key(&mut app, KeyCode::Esc);
    assert!(!app.should_quit);
    press(&mut app, "q");
    assert!(app.should_quit);
}

#[test]
fn help_opens_with_question_mark_and_closes_again() {
    let mut app = app_with_rows(3);
    for close in [KeyCode::Esc, KeyCode::Char('?'), KeyCode::Char('q')] {
        press(&mut app, "?");
        assert!(matches!(app.mode, Mode::Help(_)));
        key(&mut app, close);
        assert!(matches!(app.mode, Mode::List));
    }
    // `q` closed the popup rather than quitting.
    assert!(!app.should_quit);
}

#[test]
fn search_box_takes_every_key_while_editing() {
    let mut app = app_with_rows(3);
    press(&mut app, "/");
    assert!(app.typing == Typing::Search);
    press(&mut app, " q/jk");
    assert!(!app.should_quit);
    assert_eq!(app.query.value(), "is:pr q/jk");
    assert_eq!(app.table.selected(), Some(0));
}

#[test]
fn esc_restores_the_submitted_query() {
    let mut app = app_with_rows(3);
    press(&mut app, "/ x");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(app.typing != Typing::Search);
    assert_eq!(app.query.value(), "is:pr");
    assert_eq!(app.list_to_fetch(), None);
}

#[test]
fn up_and_down_in_the_search_box_walk_the_submitted_queries() {
    let mut app = app_with_rows(3);
    for query in ["x", "y"] {
        press(&mut app, &format!("/{query}"));
        key(&mut app, KeyCode::Enter);
    }
    press(&mut app, "/ half");
    key(&mut app, KeyCode::Up);
    assert_eq!(app.query.value(), "is:prxy");
    key(&mut app, KeyCode::Up);
    assert_eq!(app.query.value(), "is:prx");
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    // What was half typed comes back.
    assert_eq!(app.query.value(), "is:prxy half");
    assert!(app.typing == Typing::Search);
}

#[test]
fn enter_fetches_the_new_query_and_resets_the_highlight() {
    let mut app = app_with_rows(3);
    app.table.select(Some(2));
    press(&mut app, "/ is:closed");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(app.typing != Typing::Search);
    let (id, query) = app.list_to_fetch().unwrap();
    assert_eq!(query, "is:pr is:closed");
    assert!(app.list.searching);

    app.update(Msg::PrsLoaded(id, Ok(prs([7, 8]))));
    assert!(!app.list.searching);
    assert_eq!(numbers(&app), [7, 8]);
    assert_eq!(app.table.selected(), Some(0));
}

#[test]
fn results_from_an_older_fetch_are_dropped() {
    let mut app = app_with_rows(3);
    press(&mut app, "/");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
    let (old, _) = app.list_to_fetch().unwrap();
    press(&mut app, "/");
    app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
    let (new, _) = app.list_to_fetch().unwrap();

    app.update(Msg::PrsLoaded(new, Ok(prs([5]))));
    app.update(Msg::PrsLoaded(old, Ok(prs([6]))));
    assert_eq!(numbers(&app), [5]);
}

#[test]
fn labels_are_fetched_once_when_the_search_box_opens_and_branches_when_one_is_typed() {
    use super::work::NamesToFetch;
    let mut app = app_with_rows(1);
    // Not until the box is open.
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    press(&mut app, "/");
    let labels = app.names_to_fetch().labels.unwrap();
    // Nor again, while they load or after.
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    assert!(app.names.labels().is_empty());

    // Branches are left until a `base:` or `head:` value is being typed.
    press(&mut app, " label:x");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    press(&mut app, " -base:");
    let branches = app.names_to_fetch().branches.unwrap();
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    press(&mut app, " head:");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());

    app.update(Msg::LabelsLoaded(labels, Ok(vec!["deps".to_string()])));
    app.update(Msg::BranchesLoaded(branches, Ok(vec!["main".to_string()])));
    assert_eq!(app.names.labels(), ["deps"]);
    assert_eq!(app.names.branches(), ["main"]);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    press(&mut app, "/");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
}

#[test]
fn a_failed_names_fetch_is_not_retried_until_the_next_search() {
    use super::work::NamesToFetch;
    let mut app = app_with_rows(1);
    press(&mut app, "/ head:");
    let started = app.names_to_fetch();
    app.update(Msg::LabelsLoaded(
        started.labels.unwrap(),
        Err(color_eyre::eyre::eyre!("no")),
    ));
    // The other one is unaffected, and a key press doesn't ask again.
    app.update(Msg::BranchesLoaded(
        started.branches.unwrap(),
        Err(color_eyre::eyre::eyre!("no")),
    ));
    press(&mut app, "x");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());

    key(&mut app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs([1]))));
    press(&mut app, "/");
    let again = app.names_to_fetch();
    assert!(again.labels.is_some());
    // The branches wait until the cursor is in a `head:` value again, which it is.
    assert!(again.branches.is_some());
}

#[test]
fn a_names_result_that_is_not_the_latest_is_dropped() {
    let mut app = app_with_rows(1);
    press(&mut app, "/");
    let id = app.names_to_fetch().labels.unwrap();
    app.update(Msg::LabelsLoaded(id + 100, Ok(vec!["stale".to_string()])));
    assert!(app.names.labels().is_empty());
    app.update(Msg::LabelsLoaded(id, Ok(vec!["fresh".to_string()])));
    assert_eq!(app.names.labels(), ["fresh"]);
}

/// An app whose search box is open with `typed` added to the query, and whose labels are known.
fn app_completing(typed: &str) -> App {
    let mut app = app_with_rows(2);
    press(&mut app, "/");
    let started = app.names_to_fetch();
    let labels = ["deps", "bug"].map(String::from).to_vec();
    app.update(Msg::LabelsLoaded(started.labels.unwrap(), Ok(labels)));
    press(&mut app, typed);
    app
}

fn suggestions(app: &App) -> Vec<String> {
    app.completion()
        .map(|(c, _)| c.suggestions)
        .unwrap_or_default()
}

#[test]
fn no_suggestions_until_the_query_has_been_changed() {
    let mut app = app_with_rows(2);
    press(&mut app, "/");
    assert!(app.completion().is_none());
    press(&mut app, " is:p");
    assert!(app.completion().is_some());
}

#[test]
fn authors_come_from_the_rows_listed() {
    let app = app_completing(" author:");
    assert_eq!(suggestions(&app), ["alice"]);
}

#[test]
fn up_goes_through_the_history_when_there_are_no_suggestions() {
    let mut app = app_with_rows(2);
    press(&mut app, "/x");
    key(&mut app, KeyCode::Enter);
    press(&mut app, "/y");
    key(&mut app, KeyCode::Enter);
    press(&mut app, "/ la");
    assert!(app.completion().is_some());
    key(&mut app, KeyCode::Up);
    // Moved in the suggestions: the query is not recalled.
    assert_eq!(app.query.value(), "is:prxy la");
}

#[test]
fn recalling_a_query_does_not_open_a_list_that_takes_up_and_down() {
    let mut app = app_with_rows(2);
    // A recalled query that ends in something with suggestions.
    press(&mut app, "/ is:op");
    key(&mut app, KeyCode::Enter);
    press(&mut app, "/ x");
    key(&mut app, KeyCode::Up);
    assert_eq!(app.query.value(), "is:pr is:op");
    assert!(app.completion().is_none());
    key(&mut app, KeyCode::Down);
    assert_eq!(app.query.value(), "is:pr is:op x");
}

#[test]
fn esc_closes_the_list_before_it_cancels_the_edit() {
    let mut app = app_completing(" la");
    key(&mut app, KeyCode::Esc);
    assert!(app.completion().is_none());
    assert!(app.typing == Typing::Search);
    assert_eq!(app.query.value(), "is:pr la");
    // It stays closed until the text changes.
    key(&mut app, KeyCode::Left);
    key(&mut app, KeyCode::Right);
    assert!(app.completion().is_none());
    press(&mut app, "n");
    assert!(app.completion().is_some());
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Esc);
    assert!(app.typing != Typing::Search);
    assert_eq!(app.query.value(), "is:pr");
}

#[test]
fn tab_with_no_suggestions_does_nothing_to_the_query() {
    let mut app = app_completing(" zzz");
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.query.value(), "is:pr zzz");
    assert!(app.typing == Typing::Search);
}

#[test]
fn users_and_teams_are_fetched_when_a_qualifier_that_uses_them_is_typed() {
    use super::work::NamesToFetch;
    let mut app = app_with_rows(1);
    press(&mut app, "/");
    app.names_to_fetch();
    // Not for a name that is still being typed, or for a qualifier that doesn't use them.
    press(&mut app, " reviewed-by");
    press(&mut app, " author:");
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());

    press(&mut app, " reviewed-by:");
    let started = app.names_to_fetch();
    let users = started.users.unwrap();
    assert!(started.teams.is_none() && started.branches.is_none());
    assert_eq!(app.names_to_fetch(), NamesToFetch::default());
    press(&mut app, " team-review-requested:");
    let teams = app.names_to_fetch().teams.unwrap();

    app.update(Msg::UsersLoaded(users, Ok(vec!["bob".to_string()])));
    app.update(Msg::TeamsLoaded(teams, Ok(vec!["acme/web".to_string()])));
    assert_eq!(app.names.users(), ["bob"]);
    assert_eq!(app.names.teams(), ["acme/web"]);
    // The users fetched are offered with the authors of the rows.
    key(&mut app, KeyCode::Esc);
    press(&mut app, " review-requested:");
    assert_eq!(suggestions(&app), ["alice", "bob"]);
}

#[test]
fn the_list_opens_with_nothing_chosen_and_enter_still_searches() {
    let mut app = app_completing(" la");
    assert_eq!(suggestions(&app), ["label", "language"]);
    assert_eq!(app.completion().unwrap().1, None);
    key(&mut app, KeyCode::Enter);
    assert!(app.typing != Typing::Search);
    let (_, query) = app.list_to_fetch().unwrap();
    assert_eq!(query, "is:pr la");
}

#[test]
fn tab_and_down_choose_the_first_suggestion_then_the_next_round_the_ends() {
    let chosen = |app: &App| app.completion().unwrap().1;
    for next in [KeyCode::Tab, KeyCode::Down] {
        let mut app = app_completing(" la");
        key(&mut app, next);
        assert_eq!(chosen(&app), Some(0));
        key(&mut app, next);
        assert_eq!(chosen(&app), Some(1));
        key(&mut app, next);
        assert_eq!(chosen(&app), Some(0));
        // Choosing doesn't change the text.
        assert_eq!(app.query.value(), "is:pr la");
    }
}

#[test]
fn shift_tab_and_up_choose_the_first_suggestion_then_the_previous_round_the_ends() {
    let chosen = |app: &App| app.completion().unwrap().1;
    for previous in [KeyCode::BackTab, KeyCode::Up] {
        let mut app = app_completing(" la");
        key(&mut app, previous);
        assert_eq!(chosen(&app), Some(0));
        key(&mut app, previous);
        assert_eq!(chosen(&app), Some(1));
        key(&mut app, KeyCode::Tab);
        assert_eq!(chosen(&app), Some(0));
    }
}

#[test]
fn a_key_that_changes_the_text_takes_the_choice_back() {
    let mut app = app_completing(" la");
    key(&mut app, KeyCode::Tab);
    assert_eq!(app.completion().unwrap().1, Some(0));
    press(&mut app, "n");
    assert_eq!(app.completion().unwrap().1, None);
}

#[test]
fn enter_takes_the_chosen_suggestion_and_does_not_search() {
    let mut app = app_completing(" la");
    key(&mut app, KeyCode::Tab);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.query.value(), "is:pr label:");
    assert_eq!(app.query.cursor(), 12);
    assert!(app.typing == Typing::Search);
    assert_eq!(app.list_to_fetch(), None);
    // Now its values are listed, with nothing chosen, the labels the repository has sorted.
    assert_eq!(suggestions(&app), ["bug", "deps"]);
    assert_eq!(app.completion().unwrap().1, None);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.query.value(), "is:pr label:deps ");
    assert!(app.completion().is_none());
    // With nothing chosen and no list, Enter searches.
    key(&mut app, KeyCode::Enter);
    assert!(app.typing != Typing::Search);
    assert_eq!(app.list_to_fetch().unwrap().1, "is:pr label:deps ");
}

#[test]
fn all_three_boxes_take_the_word_editing_keys() {
    let alt = |code| Msg::Key(KeyEvent::new(code, KeyModifiers::ALT));
    let mut app = app_with(vec![pr_with(
        1,
        "Bump alpha",
        "OPEN",
        "",
        "MERGEABLE",
        serde_json::json!([]),
    )]);
    // The search box: Alt-Backspace deletes the last word.
    press(&mut app, "/");
    app.query = Input::new("is:pr label:deps".to_string());
    app.update(alt(KeyCode::Backspace));
    assert_eq!(app.query.value(), "is:pr ");
    app.update(Msg::Key(KeyCode::Esc.into()));
    // The filter box: Alt-B then Alt-D deletes the word the cursor moved to.
    press(&mut app, "fone two");
    app.update(alt(KeyCode::Char('b')));
    app.update(alt(KeyCode::Char('d')));
    assert_eq!(app.filter.value(), "one ");
    app.update(Msg::Key(KeyCode::Esc.into()));
    // The columns box.
    press(&mut app, "c");
    app.columns_input = Input::new("(?<a>x) beta".to_string());
    app.update(alt(KeyCode::Backspace));
    assert_eq!(app.columns_input.value(), "(?<a>x) ");
}
