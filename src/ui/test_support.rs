//! Helpers shared by the tests of the `ui` modules.

use super::*;
pub(super) use crate::app::Msg;
pub(super) use crate::app::{MIN_COLUMNS, MIN_ROWS};
pub(super) use crate::gh::Pr;
pub(super) use crate::testing::{
    PrBuilder, app_with, app_with_details, app_with_query, assert_list_mode, comment, ctrl, key,
    pr_details, press, review, select_rows, with_comment_rows,
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

/// A terminal wide enough for the wide layout, with every pane beside the list.
pub(super) const WIDE: (u16, u16) = (200, 40);

/// A terminal too narrow for the wide layout, so the panes go under the list.
pub(super) const NARROW: (u16, u16) = (100, 40);

/// Draws the app on a `WIDE` screen.
pub(super) fn draw_wide(app: &mut App) -> Buffer {
    draw(app, WIDE.0, WIDE.1)
}

/// Draws the app on a `NARROW` screen.
pub(super) fn draw_narrow(app: &mut App) -> Buffer {
    draw(app, NARROW.0, NARROW.1)
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
    draw_wide(&mut app);
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

/// Whether some row of `screen` (as `lines` gives it) has `text` in it.
pub(super) fn shows(screen: &[String], text: &str) -> bool {
    screen.iter().any(|line| line.contains(text))
}

/// The screen row that is the header of the table, taken from where the table drew its rows (so
/// that no other pane's text can be mistaken for it). It is the whole row, panes beside the table
/// included, so a column's position in it is its position on screen.
pub(super) fn table_header_line(app: &App, screen: &[String]) -> String {
    let rows = app.drawn.areas.table_rows;
    assert!(rows.y > 0, "the table has not been drawn");
    screen[usize::from(rows.y) - 1].clone()
}

/// The first screen row of the table's rows (not another pane's) with `text` in it.
pub(super) fn table_row_line(app: &App, screen: &[String], text: &str) -> String {
    let rows = app.drawn.areas.table_rows;
    screen[usize::from(rows.y)..usize::from(rows.bottom())]
        .iter()
        .find(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no table row has {text:?} in it"))
        .clone()
}

/// Whether the cell at `position` is drawn reversed, as the highlighted row or a selection is.
pub(super) fn reversed(buffer: &Buffer, position: impl Into<Position>) -> bool {
    buffer[position.into()]
        .modifier
        .contains(ratatui::style::Modifier::REVERSED)
}

/// The text of the cells of `area`'s first row.
pub(super) fn text_at(buffer: &Buffer, area: Rect) -> String {
    (area.x..area.right())
        .map(|x| buffer[(x, area.y)].symbol())
        .collect()
}

/// Which border a scrollbar is on.
#[derive(Clone, Copy)]
pub(super) enum Axis {
    /// On the right border: the thumb's rows are returned.
    Vertical,
    /// On the bottom border: the thumb's columns are returned.
    Horizontal,
}

/// The rows (for a vertical bar) or columns (horizontal) of the thumb of the scrollbar on `area`'s
/// border. The track is the border itself, so a pane has a scrollbar when it has a thumb.
pub(super) fn thumb(buffer: &Buffer, area: Rect, axis: Axis) -> Vec<u16> {
    match axis {
        Axis::Vertical => (area.y + 1..area.bottom() - 1)
            .filter(|&y| buffer[(area.right() - 1, y)].symbol() == "█")
            .collect(),
        Axis::Horizontal => (area.x + 1..area.right() - 1)
            .filter(|&x| buffer[(x, area.bottom() - 1)].symbol() == super::panes::HORIZONTAL_THUMB)
            .collect(),
    }
}

/// The layout of a body of this size with these panes hidden and resized, with three rows for the
/// approvers. It calls `pane_layout` directly, so a test sees the rects and dividers it returns.
#[allow(dead_code)] // For the `pane_layout` tests of M4 of plan.md.
pub(super) fn layout(width: u16, height: u16, hidden: &[Pane], sizes: &Sizes) -> PaneLayout {
    pane_layout(Rect::new(0, 0, width, height), 3, hidden, sizes)
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

/// The last row of the screen, the status bar, as drawn at this size.
pub(super) fn status_bar(app: &mut App, width: u16, height: u16) -> String {
    let screen = lines(&draw(app, width, height));
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
