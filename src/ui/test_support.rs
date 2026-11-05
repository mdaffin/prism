//! Helpers shared by the tests of the `ui` modules.

use super::*;
pub(super) use crate::app::Msg;
pub(super) use crate::app::{MIN_COLUMNS, MIN_ROWS};
pub(super) use crate::gh::Pr;
pub(super) use crate::testing::{
    PrBuilder, app_with, app_with_details, app_with_query, comment, ctrl, ctrl_g, key, pr_details,
    press, select_rows, with_comment_rows,
};
pub(super) use crossterm::event::KeyCode;
pub(super) use ratatui::Terminal;
pub(super) use ratatui::backend::TestBackend;
pub(super) use ratatui::style::Color;

pub(super) fn pr(number: u64, title: &str, state: &str, review: &str) -> Pr {
    PrBuilder::new(number)
        .title(title)
        .state(state)
        .review(review)
        .build()
}

/// Draws the app on a screen of this size, as the real terminal would show it.
pub(super) fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view(app, frame)).unwrap();
    terminal.backend().buffer().clone()
}

/// The text of each screen row.
pub(super) fn lines(buffer: &Buffer) -> Vec<String> {
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect()
}

/// The position of the first cell of the first occurrence of `text`. Read its colours with
/// `buffer[position]`.
pub(super) fn find(buffer: &Buffer, text: &str) -> Option<Position> {
    lines(buffer).iter().enumerate().find_map(|(y, line)| {
        let column = line.find(text).map(|byte| line[..byte].chars().count())?;
        Some(Position::new(column as u16, y as u16))
    })
}

pub(super) fn run(name: &str, status: &str, conclusion: &str) -> serde_json::Value {
    serde_json::json!({
        "__typename": "CheckRun",
        "name": name,
        "workflowName": "CI",
        "status": status,
        "conclusion": conclusion,
        "startedAt": "2026-01-02T03:00:00Z",
    })
}

pub(super) fn pr_checked(number: u64, mergeable: &str, checks: Vec<serde_json::Value>) -> Pr {
    PrBuilder::new(number)
        .title(&format!("PR {number}"))
        .mergeable(mergeable)
        .checks(checks.into())
        .build()
}

/// Five open PRs, one in each state. The table puts them in this order: passing first, then
/// pending and none, then failing and conflict.
pub(super) fn one_of_each() -> App {
    app_with(vec![
        pr_checked(1, "MERGEABLE", vec![run("build", "COMPLETED", "SUCCESS")]),
        pr_checked(2, "MERGEABLE", vec![run("build", "IN_PROGRESS", "")]),
        pr_checked(3, "MERGEABLE", vec![]),
        pr_checked(4, "MERGEABLE", vec![run("build", "COMPLETED", "FAILURE")]),
        pr_checked(5, "CONFLICTING", vec![run("build", "COMPLETED", "SUCCESS")]),
    ])
}

/// A PR by `login`, and an app that has loaded these PRs for `query`.
pub(super) fn by(login: &str, number: u64, title: &str) -> Pr {
    PrBuilder::new(number).title(title).author(login).build()
}

pub(super) fn wheel(app: &mut App, position: Position, down: bool) {
    use crossterm::event::MouseEventKind;
    let kind = if down {
        MouseEventKind::ScrollDown
    } else {
        MouseEventKind::ScrollUp
    };
    mouse(app, kind, position);
}

/// A scroll event of `kind` with `modifiers` held, at `position`.
pub(super) fn scroll_at(
    app: &mut App,
    kind: crossterm::event::MouseEventKind,
    modifiers: crossterm::event::KeyModifiers,
    position: Position,
) {
    app.update(Msg::Mouse(crossterm::event::MouseEvent {
        kind,
        column: position.x,
        row: position.y,
        modifiers,
    }));
}

/// Clicks the row at `index` of the rows drawn (so `0` is the first one on screen), `dx` columns
/// in from the left edge of the table.
pub(super) fn click_table_row(app: &mut App, index: u16, dx: u16) {
    let rows = app.drawn.areas.table_rows;
    press_at(app, Position::new(rows.x + dx, rows.y + index));
}

/// Clicks the checkbox of the row at `index` of the rows drawn.
pub(super) fn click_checkbox(app: &mut App, index: u16) {
    let boxes = app.drawn.areas.table_checkboxes;
    press_at(app, Position::new(boxes.x + 1, boxes.y + index));
}

/// An app whose highlighted PR has enough checks, comments, description and diff to scroll.
pub(super) fn app_with_long_panes() -> App {
    let checks = (0..40)
        .map(|i| run(&format!("check {i}"), "COMPLETED", "SUCCESS"))
        .collect();
    let mut app = app_with(vec![
        pr_checked(1, "MERGEABLE", checks),
        pr_checked(2, "MERGEABLE", vec![]),
        pr_checked(3, "MERGEABLE", vec![]),
    ]);
    let long = vec!["a line"; 100].join("\n\n");
    let comment = comment("someone", &long);
    let details = pr_details(&long, serde_json::json!([comment, comment, comment]));
    app.details_loaded(1, Ok(details));
    app.diff_loaded(1, Ok(vec!["+added"; 200].join("\n")));
    draw(&mut app, 200, 40);
    app
}

/// A mouse event of `kind` at `position`.
pub(super) fn mouse(app: &mut App, kind: crossterm::event::MouseEventKind, position: Position) {
    app.update(Msg::Mouse(crossterm::event::MouseEvent {
        kind,
        column: position.x,
        row: position.y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }));
}

pub(super) fn press_at(app: &mut App, position: Position) {
    use crossterm::event::{MouseButton, MouseEventKind};
    mouse(app, MouseEventKind::Down(MouseButton::Left), position);
}

pub(super) fn drag_to(app: &mut App, position: Position) {
    use crossterm::event::{MouseButton, MouseEventKind};
    mouse(app, MouseEventKind::Drag(MouseButton::Left), position);
}

pub(super) fn release_at(app: &mut App, position: Position) {
    use crossterm::event::{MouseButton, MouseEventKind};
    mouse(app, MouseEventKind::Up(MouseButton::Left), position);
}

/// The scrollbar drawn for `target`.
pub(super) fn bar(app: &App, target: ScrollTarget) -> ScrollbarHit {
    *app.drawn
        .areas
        .scrollbars
        .iter()
        .find(|hit| hit.target == target)
        .unwrap_or_else(|| panic!("no scrollbar for {target:?}"))
}

/// Whether `area`'s right border has a scrollbar (its track or thumb) beside the first row.
pub(super) fn has_scrollbar(buffer: &Buffer, area: Rect) -> bool {
    matches!(buffer[(area.right() - 1, area.y + 1)].symbol(), "█" | "║")
}

/// The text of row `row` (counted from the first inside the border) of the pane at `area`.
pub(super) fn pane_row(buffer: &Buffer, area: Rect, row: u16) -> String {
    (area.x + 1..area.right() - 1)
        .map(|x| buffer[(x, area.y + 1 + row)].symbol())
        .collect()
}

pub(super) fn focus(app: &mut App, pane: Pane) {
    while app.focus != pane {
        key(app, KeyCode::Tab);
    }
}

pub(super) fn app_with_a_long_title() -> App {
    let title = format!("{}END", "long ".repeat(40));
    app_with(vec![by("alice", 1, &title), by("bob", 2, "Short")])
}

pub(super) fn status_bar(app: &mut App, width: u16) -> String {
    let screen = lines(&draw(app, width, 30));
    screen.last().unwrap().trim_end().to_string()
}

/// What a failed `gh pr view` gives: a one-line message, then more.
pub(super) fn gh_failure() -> color_eyre::Report {
    crate::gh::GhError {
        command: "gh pr view 1 --json headRefName,comments".to_string(),
        output: "GraphQL: Could not resolve to a PullRequest with the number of 1.\nmore detail that is not for the pane".to_string(),
    }
    .into()
}
