//! Helpers shared by the tests of `app` and `ui`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::app::{App, Msg};
use crate::gh::Pr;

/// Builds a `Pr` as gh lists it. `PrBuilder::new(number)` is an open, mergeable PR titled "t" by
/// alice, with no review and no checks, updated on 2026-01-01: set only what a test cares about.
pub struct PrBuilder {
    number: u64,
    title: String,
    state: String,
    review: String,
    mergeable: String,
    rollup: Value,
    author: String,
    updated_at: String,
}

impl PrBuilder {
    pub fn new(number: u64) -> Self {
        PrBuilder {
            number,
            title: "t".to_string(),
            state: "OPEN".to_string(),
            review: String::new(),
            mergeable: "MERGEABLE".to_string(),
            rollup: json!([]),
            author: "alice".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    pub fn title(mut self, title: &str) -> Self {
        self.title = title.to_string();
        self
    }

    /// `"OPEN"`, `"CLOSED"` or `"MERGED"`.
    pub fn state(mut self, state: &str) -> Self {
        self.state = state.to_string();
        self
    }

    /// The `reviewDecision`: `""`, `"APPROVED"` or `"CHANGES_REQUESTED"`.
    pub fn review(mut self, review: &str) -> Self {
        self.review = review.to_string();
        self
    }

    /// `"MERGEABLE"` or `"CONFLICTING"`.
    pub fn mergeable(mut self, mergeable: &str) -> Self {
        self.mergeable = mergeable.to_string();
        self
    }

    /// The `statusCheckRollup`: a JSON array of check runs and status contexts.
    pub fn checks(mut self, rollup: Value) -> Self {
        self.rollup = rollup;
        self
    }

    pub fn author(mut self, login: &str) -> Self {
        self.author = login.to_string();
        self
    }

    pub fn updated_at(mut self, updated_at: &str) -> Self {
        self.updated_at = updated_at.to_string();
        self
    }

    pub fn build(self) -> Pr {
        serde_json::from_value(json!({
            "number": self.number,
            "title": self.title,
            "state": self.state,
            "statusCheckRollup": self.rollup,
            "mergeable": self.mergeable,
            "reviewDecision": self.review,
            "url": format!("https://example.test/pull/{}", self.number),
            "author": {"login": self.author},
            "updatedAt": self.updated_at,
        }))
        .unwrap()
    }
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

/// Presses `code`.
pub fn key(app: &mut App, code: KeyCode) {
    app.update(Msg::Key(code.into()));
}

/// Presses Ctrl and the character `c`.
pub fn ctrl(app: &mut App, c: char) {
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL,
    )));
}

/// Presses Ctrl-Enter, which reviews the selected PRs like Ctrl-G.
pub fn ctrl_enter(app: &mut App) {
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::CONTROL,
    )));
}

/// Presses Ctrl-G, which reviews the selected PRs.
pub fn ctrl_g(app: &mut App) {
    ctrl(app, 'g');
}

/// A completed, successful check run, as `statusCheckRollup` lists it.
pub fn passing_run() -> Value {
    json!({
        "__typename": "CheckRun", "name": "build", "workflowName": "CI",
        "status": "COMPLETED", "conclusion": "SUCCESS", "startedAt": null,
    })
}

/// A commit status from "ci" in `state`: `"SUCCESS"`, `"PENDING"` or `"FAILURE"`.
pub fn status_context(state: &str) -> Value {
    json!({"__typename": "StatusContext", "context": "ci", "state": state})
}
