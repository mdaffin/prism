//! Helpers shared by the tests of `app` and `ui`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};

use crate::app::{App, ChecksLayout, CommentLayout, Mode, Msg};
use crate::gh::{Pr, PrDetails};

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
    app_with_query("is:pr", prs)
}

/// Like `app_with`, for a search of `query`.
pub fn app_with_query(query: &str, prs: Vec<Pr>) -> App {
    let mut app = App::new("PRs".to_string(), query.to_string(), false);
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

/// A comment by `author` with this body, made at 2026-01-02 03:00. Add `url`, `kind` or `replies`
/// to the JSON for the ones that matter.
pub fn comment(author: &str, body: &str) -> Value {
    json!({
        "author": {"login": author},
        "createdAt": "2026-01-02T03:00:00Z",
        "body": body,
    })
}

/// The details of a PR on the branch "branch", with +1 -1 lines and no approvers. `comments` is a
/// JSON array of `comment(..)`s or hand-written ones.
pub fn pr_details(body: &str, comments: Value) -> PrDetails {
    serde_json::from_value(json!({
        "headRefName": "branch",
        "body": body,
        "comments": comments,
        "additions": 1,
        "deletions": 1,
    }))
    .unwrap()
}

/// An app that has loaded `prs`, and then the `details` of the first one.
pub fn app_with_details(prs: Vec<Pr>, details: PrDetails) -> App {
    let number = prs[0].number;
    let mut app = app_with(prs);
    app.details_loaded(number, Ok(details));
    app
}

/// Searches again (`/`, Enter) and delivers `prs` as the result.
pub fn search_and_load(app: &mut App, prs: Vec<Pr>) {
    press(app, "/");
    key(app, KeyCode::Enter);
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, Ok(prs)));
}

/// Refreshes the list with Ctrl-R and delivers `result`.
pub fn refresh_and_load(app: &mut App, result: color_eyre::Result<Vec<Pr>>) {
    ctrl(app, 'r');
    let (id, _) = app.list_to_fetch().unwrap();
    app.update(Msg::PrsLoaded(id, result));
}

/// Selects the rows at these positions of the visible list, as Space and `j` would, and leaves
/// the highlight on the last of them.
pub fn select_rows(app: &mut App, rows: &[usize]) {
    for &row in rows {
        while app.table.selected() != Some(row) {
            let before = app.table.selected();
            press(app, if before < Some(row) { "j" } else { "k" });
        }
        press(app, " ");
    }
}

/// Records that the comments pane drew comments of these heights (header included) one under the
/// other, in a viewport of 5 lines, as `update` needs to keep the highlighted one on screen.
pub fn with_comment_rows(app: &mut App, heights: &[usize]) {
    let mut start = 0;
    let items = heights
        .iter()
        .map(|&height| {
            let item = (start, height);
            start += height;
            item
        })
        .collect();
    app.drawn.comment_layout = CommentLayout { items, viewport: 5 };
}

/// Records that the checks pane drew `count` checks below `first` other rows (1 for a conflict
/// row, else 0), in a viewport of `viewport` rows.
pub fn with_check_rows(app: &mut App, count: usize, first: usize, viewport: usize) {
    app.drawn.checks_layout = ChecksLayout {
        count,
        first,
        viewport,
    };
}

/// Asserts that no dialog is open.
pub fn assert_list_mode(app: &App) {
    assert!(
        matches!(app.mode, Mode::List),
        "a dialog is open, not the list"
    );
}
