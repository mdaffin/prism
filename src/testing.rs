//! Helpers shared by the tests of `app` and `ui`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::app::{App, Msg};
use crate::gh::Pr;

/// A PR as gh lists it. `rollup` is its `statusCheckRollup`.
pub fn pr_with(
    number: u64,
    title: &str,
    state: &str,
    review: &str,
    mergeable: &str,
    rollup: Value,
) -> Pr {
    serde_json::from_value(json!({
        "number": number,
        "title": title,
        "state": state,
        "statusCheckRollup": rollup,
        "mergeable": mergeable,
        "reviewDecision": review,
        "url": format!("https://example.test/pull/{number}"),
        "author": {"login": "alice"},
        "updatedAt": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

/// An app that has loaded these PRs, in the order given before the table sorts them.
pub fn app_with(prs: Vec<Pr>) -> App {
    let mut app = App::new("PRs".to_string(), "is:pr".to_string(), false);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs)));
    app
}

/// Presses a key for each character of `keys`.
pub fn press(app: &mut App, keys: &str) {
    for c in keys.chars() {
        app.update(Msg::Key(KeyCode::Char(c).into()));
    }
}

/// Presses Ctrl-G, which reviews the selected PRs.
pub fn ctrl_g(app: &mut App) {
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Char('g'),
        KeyModifiers::CONTROL,
    )));
}
