use std::collections::HashSet;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Margin, Position, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Table, TableState, Wrap,
};
use regex::Regex;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{
    self, App, ChecksLayout, CommentLayout, ConfirmOpen, Details, Divider, Failure, Fetch,
    MIN_WEIGHT, Mode, Page, Pane, PopupView, ScrollTarget, ScrollbarHit, Sizes, StackId, TextRow,
    TextView, Typing, to_u16,
};
use crate::batch::{Batch, Confirm, ItemStatus, Summary};
use crate::gh::{Approvers, Comment, PrState, Reviewer, Verdict};
use crate::keys;
use crate::model::{self, CheckState, Column, Sort};

mod panes;
mod popups;
mod status;
mod table;

use panes::*;
use popups::*;
use status::*;
use table::*;

/// The most rows a list fetch error takes above the status bar.
const MAX_ERROR_ROWS: u16 = 5;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The rows of the search and filter boxes: a line of text and the border around it.
const INPUT_BOX_ROWS: u16 = 3;

/// The narrowest terminal that gets the list beside the details. The table needs about 90 columns
/// for its fixed columns and a title that reads, and it is half the screen.
const WIDE_LAYOUT_MIN_WIDTH: u16 = 180;

/// The most rows the comments pane takes, and the checks and comments in a narrow terminal.
const MAX_COMMENTS_ROWS: u16 = 10;

/// The fewest rows the checks and comments get in a narrow terminal, so a short one still shows a
/// few of each.
const MIN_LOWER_ROWS: u16 = 6;

/// The rows of the Pull request box: the border, the title and the summary line.
const PULL_REQUEST_ROWS: u16 = 4;

/// The most rows the approvers pane takes, its border included, and the fewest. In between it fits
/// its lines, if the room allows.
const MAX_APPROVERS_ROWS: u16 = 8;

const MIN_APPROVERS_ROWS: u16 = 3;

/// Where each part of the screen goes.
struct PaneLayout {
    /// The search and filter boxes and the table.
    list: Rect,
    checks: Rect,
    /// Above the diff.
    approvers: Rect,
    comments: Rect,
    pull_request: Rect,
    description: Rect,
    diff: Rect,
    /// The borders between panes, for the mouse and the resize keys.
    dividers: Vec<Divider>,
}

/// Splits `area` between the `items` that are on, each with its constraint, down or across, and
/// gives each item its piece. An item that is off gets an empty rectangle and no room.
fn split(area: Rect, vertical: bool, items: &[(bool, Constraint)]) -> Vec<Rect> {
    let on: Vec<Constraint> = items
        .iter()
        .filter(|(on, _)| *on)
        .map(|&(_, constraint)| constraint)
        .collect();
    let layout = if vertical {
        Layout::vertical(on)
    } else {
        Layout::horizontal(on)
    };
    let mut pieces = layout.split(area).to_vec().into_iter();
    items
        .iter()
        .map(|(on, _)| {
            if *on {
                pieces.next().unwrap_or_default()
            } else {
                Rect::default()
            }
        })
        .collect()
}

/// The approvers over the diff, in `area`, for whichever of the two are shown. The approvers get
/// `wanted` rows, but never more than a third of the area.
fn approvers_over_diff(area: Rect, wanted: u16, (approvers, diff): (bool, bool)) -> (Rect, Rect) {
    let rows = wanted.min(area.height / 3);
    let pieces = split(
        area,
        true,
        &[
            (
                approvers,
                if diff {
                    Constraint::Length(rows)
                } else {
                    Constraint::Fill(1)
                },
            ),
            (diff, Constraint::Fill(1)),
        ],
    );
    (pieces[0], pieces[1])
}

/// The constraint of item `index` of `stack`: its weight once the stack has been resized, else
/// `default`.
fn sized(sizes: &Sizes, stack: StackId, index: usize, default: Constraint) -> Constraint {
    match sizes.of(stack) {
        Some(weights) => Constraint::Fill(weights[index].max(MIN_WEIGHT)),
        None => default,
    }
}

/// One item of a stack of panes: its number in the stack, the panes in it, and where it is (empty
/// when it is not shown).
struct Item<'a> {
    index: usize,
    panes: &'a [Pane],
    rect: Rect,
}

/// The borders between the items of a stack that are shown, `stacked` one over the other or side
/// by side, for the mouse and the resize keys.
fn dividers_of(stack: StackId, stacked: bool, panes_and_rects: &[(&[Pane], Rect)]) -> Vec<Divider> {
    let items: Vec<Item> = panes_and_rects
        .iter()
        .enumerate()
        .map(|(index, &(panes, rect))| Item { index, panes, rect })
        .collect();
    let shown: Vec<&Item> = items.iter().filter(|item| !item.rect.is_empty()).collect();
    let length = |item: &Item| {
        if stacked {
            item.rect.height
        } else {
            item.rect.width
        }
    };
    let first = |item: &Item| if stacked { item.rect.y } else { item.rect.x };
    shown
        .windows(2)
        .map(|pair| {
            let (a, b) = (pair[0], pair[1]);
            // The two borders that meet there: a's last row or column and b's first.
            let hit = if stacked {
                Rect::new(a.rect.x, a.rect.bottom() - 1, a.rect.width, 2)
            } else {
                Rect::new(a.rect.right() - 1, a.rect.y, 2, a.rect.height)
            };
            Divider {
                stack,
                stacked,
                hit,
                between: (a.index, b.index),
                before: a.panes.to_vec(),
                after: b.panes.to_vec(),
                items: shown
                    .iter()
                    .map(|item| (item.index, length(item)))
                    .collect(),
                boundary: first(b),
                start: first(a),
                end: first(b) + length(b),
            }
        })
        .collect()
}

/// Which of the panes that can be hidden are shown.
struct Shown {
    checks: bool,
    comments: bool,
    description: bool,
    approvers: bool,
    diff: bool,
}

impl Shown {
    fn new(hidden: &[Pane]) -> Shown {
        let shown = |pane| !hidden.contains(&pane);
        Shown {
            checks: shown(Pane::Checks),
            comments: shown(Pane::Comments),
            description: shown(Pane::Description),
            approvers: shown(Pane::Approvers),
            diff: shown(Pane::Diff),
        }
    }

    /// Whether any pane of the PR's details is shown.
    fn details(&self) -> bool {
        self.description || self.right()
    }

    /// Whether the approvers or the diff, which share a column, are shown.
    fn right(&self) -> bool {
        self.approvers || self.diff
    }

    /// Whether the checks or the comments are shown.
    fn lower(&self) -> bool {
        self.checks || self.comments
    }
}

/// With room, the list is above the checks and comments on the left and the PR's details are on
/// the right. In a narrow terminal that would leave the table too little width, so the list takes
/// the full width on top. Under it, what is read most gets the space: the description beside the
/// diff, with the checks and comments in a short row at the bottom. The approvers are in the
/// details, above the diff, and take `approvers_rows` if they can. A pane in `hidden` is left out
/// (its rectangle is empty) and the others share its room; with all of a side's panes hidden, the
/// other side takes the width, and the Pull request box goes with the details. A stack that has
/// been resized (`sizes`) shares its room by the weights it has, and every border between two
/// panes is returned in `dividers`.
fn pane_layout(body: Rect, approvers_rows: u16, hidden: &[Pane], sizes: &Sizes) -> PaneLayout {
    let shown = Shown::new(hidden);
    let fill = Constraint::Fill(1);
    if body.width >= WIDE_LAYOUT_MIN_WIDTH {
        let sides = split(
            body,
            false,
            &[
                (true, sized(sizes, StackId::Columns, 0, fill)),
                (shown.details(), sized(sizes, StackId::Columns, 1, fill)),
            ],
        );
        let left = split(
            sides[0],
            true,
            &[
                (true, sized(sizes, StackId::Left, 0, Constraint::Fill(2))),
                (shown.checks, sized(sizes, StackId::Left, 1, fill)),
                (
                    shown.comments,
                    sized(sizes, StackId::Left, 2, Constraint::Max(MAX_COMMENTS_ROWS)),
                ),
            ],
        );
        let detail = split(
            sides[1],
            true,
            &[
                (shown.details(), Constraint::Length(PULL_REQUEST_ROWS)),
                (shown.description, sized(sizes, StackId::Detail, 0, fill)),
                (
                    shown.approvers || shown.diff,
                    sized(sizes, StackId::Detail, 1, Constraint::Fill(2)),
                ),
            ],
        );
        let (approvers, diff) =
            approvers_over_diff(detail[2], approvers_rows, (shown.approvers, shown.diff));
        let mut dividers = dividers_of(
            StackId::Columns,
            false,
            &[
                (&[Pane::Table, Pane::Checks, Pane::Comments], sides[0]),
                (&[Pane::Description, Pane::Approvers, Pane::Diff], sides[1]),
            ],
        );
        dividers.extend(dividers_of(
            StackId::Left,
            true,
            &[
                (&[Pane::Table], left[0]),
                (&[Pane::Checks], left[1]),
                (&[Pane::Comments], left[2]),
            ],
        ));
        dividers.extend(dividers_of(
            StackId::Detail,
            true,
            &[
                (&[Pane::Description], detail[1]),
                (&[Pane::Approvers, Pane::Diff], detail[2]),
            ],
        ));
        PaneLayout {
            list: left[0],
            checks: left[1],
            approvers,
            comments: left[2],
            pull_request: detail[0],
            description: detail[1],
            diff,
            dividers,
        }
    } else {
        let lower_rows = (body.height / 5).clamp(MIN_LOWER_ROWS, MAX_COMMENTS_ROWS);
        let rows = split(
            body,
            true,
            &[
                (true, sized(sizes, StackId::Rows, 0, Constraint::Fill(2))),
                (
                    shown.details(),
                    sized(sizes, StackId::Rows, 1, Constraint::Fill(3)),
                ),
                (
                    shown.lower(),
                    sized(sizes, StackId::Rows, 2, Constraint::Length(lower_rows)),
                ),
            ],
        );
        let columns = split(
            rows[1],
            false,
            &[
                (
                    shown.description,
                    sized(sizes, StackId::DetailColumns, 0, fill),
                ),
                (shown.right(), sized(sizes, StackId::DetailColumns, 1, fill)),
            ],
        );
        // The Pull request box is over the description, or over the diff if that is hidden.
        let left = split(
            columns[0],
            true,
            &[
                (shown.description, Constraint::Length(PULL_REQUEST_ROWS)),
                (shown.description, Constraint::Fill(1)),
            ],
        );
        let right = split(
            columns[1],
            true,
            &[
                (
                    shown.right() && !shown.description,
                    Constraint::Length(PULL_REQUEST_ROWS),
                ),
                (shown.right(), Constraint::Fill(1)),
            ],
        );
        let (approvers, diff) =
            approvers_over_diff(right[1], approvers_rows, (shown.approvers, shown.diff));
        let lower = split(
            rows[2],
            false,
            &[
                (shown.checks, sized(sizes, StackId::Lower, 0, fill)),
                (shown.comments, sized(sizes, StackId::Lower, 1, fill)),
            ],
        );
        let mut dividers = dividers_of(
            StackId::Rows,
            true,
            &[
                (&[Pane::Table], rows[0]),
                (&[Pane::Description, Pane::Approvers, Pane::Diff], rows[1]),
                (&[Pane::Checks, Pane::Comments], rows[2]),
            ],
        );
        dividers.extend(dividers_of(
            StackId::DetailColumns,
            false,
            &[
                (&[Pane::Description], columns[0]),
                (&[Pane::Approvers, Pane::Diff], columns[1]),
            ],
        ));
        dividers.extend(dividers_of(
            StackId::Lower,
            false,
            &[(&[Pane::Checks], lower[0]), (&[Pane::Comments], lower[1])],
        ));
        PaneLayout {
            list: rows[0],
            checks: lower[0],
            approvers,
            comments: lower[1],
            pull_request: if shown.description { left[0] } else { right[0] },
            description: left[1],
            diff,
            dividers,
        }
    }
}

pub fn view(app: &mut App, frame: &mut Frame) {
    app.drawn.areas.scrollbars.clear();
    let error = app.errors.text();
    let error_rows = error
        .as_deref()
        .map_or(0, |error| wrapped_rows(error, frame.area().width));
    let [title_area, body_area, error_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(error_rows),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let PaneLayout {
        list: top_area,
        checks: checks_area,
        approvers: approvers_area,
        comments: comments_area,
        pull_request: pull_request_area,
        description: description_area,
        diff: diff_area,
        dividers,
    } = pane_layout(
        body_area,
        approvers_rows(app, body_area.width / 2),
        &app.hidden_panes,
        &app.sizes,
    );
    // The filter box is only there while it is being edited or has text in it.
    let filter_rows = if app.typing == Typing::Filter || !app.filter.value().is_empty() {
        INPUT_BOX_ROWS
    } else {
        0
    };
    // The columns box is only there while it is being edited.
    let columns_rows = if app.typing == Typing::Columns {
        INPUT_BOX_ROWS
    } else {
        0
    };
    let [search_area, filter_area, columns_area, table_area] = Layout::vertical([
        Constraint::Length(INPUT_BOX_ROWS),
        Constraint::Length(filter_rows),
        Constraint::Length(columns_rows),
        Constraint::Fill(1),
    ])
    .areas(top_area);

    frame.render_widget(Line::from(app.title()).bold().reversed(), title_area);
    app.drawn.areas.search = search_area;
    app.drawn.areas.dividers = dividers;
    app.drawn.areas.filter = none_if_empty(filter_area);
    app.drawn.areas.columns = none_if_empty(columns_area);
    search(app, frame, search_area);
    if filter_rows > 0 {
        filter(app, frame, filter_area);
    }
    if columns_rows > 0 {
        columns(app, frame, columns_area);
    }

    pr_table(app, frame, table_area);
    draw_pane(app, frame, Pane::Checks, checks_area, checks);
    draw_pane(app, frame, Pane::Approvers, approvers_area, approvers);
    draw_pane(app, frame, Pane::Comments, comments_area, comments);
    if !pull_request_area.is_empty() {
        pull_request(app, frame, pull_request_area);
    }
    draw_pane(app, frame, Pane::Description, description_area, description);
    draw_pane(app, frame, Pane::Diff, diff_area, diff);

    if let Some(error) = error {
        let error = Paragraph::new(error).red().wrap(Wrap { trim: false });
        frame.render_widget(error, error_area);
    }
    suggestions(app, frame, search_area, body_area);
    draw_dialog(app, frame, body_area);
    let status = status_line(app, status_area.width);
    frame.render_widget(
        Line::from(status).style(Style::new().reversed()),
        status_area,
    );
}

/// Draws a pane in `area`, unless it is hidden. A hidden pane is not drawn, and what it recorded of
/// itself is cleared so that nothing acts on a pane that is not there.
fn draw_pane(
    app: &mut App,
    frame: &mut Frame,
    pane: Pane,
    area: Rect,
    draw: fn(&mut App, &mut Frame, Rect),
) {
    if app.pane_hidden(pane) {
        app.drawn.forget(pane);
    } else {
        draw(app, frame, area);
    }
}

/// `area`, or the default empty one if it has no room, so that a stale rectangle can't catch a
/// click.
fn none_if_empty(area: Rect) -> Rect {
    if area.is_empty() {
        Rect::default()
    } else {
        area
    }
}

/// Green for passing, yellow for pending or no checks, red for failing or a conflict.
fn state_style(state: CheckState) -> Style {
    match state.rank() {
        0 => Style::new().green(),
        1 => Style::new().yellow(),
        _ => Style::new().red(),
    }
}

/// The spinner's current frame.
fn spinner(app: &App) -> &'static str {
    SPINNER[app.spinner % SPINNER.len()]
}

/// `1 PR`, `3 PRs`.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// About how many rows `text` takes when wrapped to `width`, at most [`MAX_ERROR_ROWS`].
fn wrapped_rows(text: &str, width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let rows: usize = text
        .lines()
        .map(|line| line.chars().count().div_ceil(width).max(1))
        .sum();
    rows.min(usize::from(MAX_ERROR_ROWS)) as u16
}

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::*;

    #[test]
    fn a_click_focuses_the_pane_under_it_in_a_wide_and_a_narrow_terminal() {
        for (width, height) in [WIDE, NARROW] {
            let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
            draw(&mut app, width, height);
            let areas = &app.drawn.areas;
            let panes = [
                (Pane::Checks, areas.checks),
                (Pane::Approvers, areas.approvers),
                (Pane::Comments, areas.comments),
                (Pane::Description, areas.description),
                (Pane::Diff, areas.diff),
                (Pane::Table, areas.table),
            ];
            for (pane, area) in panes {
                // The border counts as part of the pane, so click on it and inside it.
                for (dx, dy) in [(0, 0), (4, 0), (2, 1)] {
                    app.focus = Pane::Table;
                    press_at(&mut app, Position::new(area.x + dx, area.y + dy));
                    assert_eq!(app.focus, pane, "{pane:?} at {dx},{dy} in {width}x{height}");
                }
            }
            // The search box and the title are in no pane.
            app.focus = Pane::Diff;
            press_at(&mut app, Position::new(0, 0));
            assert_eq!(app.focus, Pane::Diff);
        }
    }

    /// A point inside each pane.
    struct Points {
        table: Position,
        checks: Position,
        description: Position,
        diff: Position,
        comments: Position,
    }

    /// The areas of the panes that can be hidden, in `Pane::HIDEABLE` order.
    fn hideable_areas(app: &App) -> Vec<Rect> {
        let a = &app.drawn.areas;
        vec![a.checks, a.comments, a.description, a.approvers, a.diff]
    }

    /// The widths of the screens to try, one with each layout.
    const BOTH_LAYOUTS: [u16; 2] = [200, 100];

    fn area_of(r: &Rect) -> u32 {
        u32::from(r.width) * u32::from(r.height)
    }

    /// The area the panes and the list fill together.
    fn shown_total(app: &App) -> u32 {
        hideable_areas(app).iter().map(area_of).sum::<u32>() + area_of(&app.drawn.areas.table)
    }

    /// An app drawn at `width` by 40 with every pane shown.
    fn app_with_every_pane(width: u16) -> App {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw(&mut app, width, 40);
        app
    }

    #[test]
    fn every_pane_is_shown_to_begin_with_in_both_layouts() {
        for width in BOTH_LAYOUTS {
            let app = app_with_every_pane(width);
            let all = hideable_areas(&app);
            assert!(all.iter().all(|r| !r.is_empty()), "{width}: {all:?}");
        }
    }

    #[test]
    fn hiding_a_pane_empties_that_pane_and_no_other() {
        for width in BOTH_LAYOUTS {
            for (i, pane) in Pane::HIDEABLE.into_iter().enumerate() {
                let mut app = app_with_every_pane(width);
                press(&mut app, &(i + 1).to_string());
                draw(&mut app, width, 40);
                for (j, area) in hideable_areas(&app).iter().enumerate() {
                    assert_eq!(
                        area.is_empty(),
                        j == i,
                        "{width}: hiding {pane:?}: pane {j} is {area:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_room_a_hidden_pane_had_goes_to_the_others() {
        for width in BOTH_LAYOUTS {
            for (i, pane) in Pane::HIDEABLE.into_iter().enumerate() {
                let mut app = app_with_every_pane(width);
                let before = shown_total(&app);
                let had = area_of(&hideable_areas(&app)[i]);
                press(&mut app, &(i + 1).to_string());
                draw(&mut app, width, 40);
                // The panes still fill about as much of the screen, give or take its borders.
                let kept = shown_total(&app);
                assert!(kept + had / 2 > before, "{width}: {pane:?} {kept} {before}");
            }
        }
    }

    #[test]
    fn panes_never_overlap_when_one_is_hidden() {
        for width in BOTH_LAYOUTS {
            for (i, pane) in Pane::HIDEABLE.into_iter().enumerate() {
                let mut app = app_with_every_pane(width);
                press(&mut app, &(i + 1).to_string());
                draw(&mut app, width, 40);
                let mut drawn = hideable_areas(&app);
                drawn.push(app.drawn.areas.table);
                for (a, ra) in drawn.iter().enumerate() {
                    for rb in &drawn[a + 1..] {
                        assert!(
                            ra.intersection(*rb).is_empty(),
                            "{width}: hiding {pane:?}: {ra:?} {rb:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_hidden_pane_is_back_when_shown_again() {
        for width in BOTH_LAYOUTS {
            for (i, pane) in Pane::HIDEABLE.into_iter().enumerate() {
                let mut app = app_with_every_pane(width);
                press(&mut app, &(i + 1).to_string());
                press(&mut app, &(i + 1).to_string());
                draw(&mut app, width, 40);
                assert!(!hideable_areas(&app)[i].is_empty(), "{width}: {pane:?}");
            }
        }
    }
    #[test]
    fn hiding_every_pane_leaves_the_list_across_the_whole_screen_and_0_brings_them_back() {
        for width in [200u16, 100] {
            let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
            press(&mut app, "12345");
            let screen = lines(&draw(&mut app, width, 40));
            assert!(hideable_areas(&app).iter().all(|r| r.is_empty()), "{width}");
            assert_eq!(app.drawn.areas.table.width, width, "{width}");
            assert!(
                !screen.join("\n").contains("Pull request\u{2500}"),
                "{width}"
            );
            press(&mut app, "0");
            draw(&mut app, width, 40);
            assert!(
                hideable_areas(&app).iter().all(|r| !r.is_empty()),
                "{width}"
            );
            assert!(app.hidden_panes.is_empty());
        }
    }

    #[test]
    fn hiding_the_focused_pane_moves_the_focus_and_tab_skips_hidden_panes() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw_wide(&mut app);
        // Focus the checks, then hide them: the focus moves on to the next shown pane.
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Pane::Checks);
        press(&mut app, "1");
        assert_eq!(app.focus, Pane::Comments);
        // Tab from the list now goes straight to the comments, and Shift-Tab round to the diff.
        key(&mut app, KeyCode::BackTab);
        key(&mut app, KeyCode::BackTab);
        assert_eq!(app.focus, Pane::Diff);
        press(&mut app, "5");
        assert_eq!(app.focus, Pane::Table);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Pane::Comments);
        // A hidden pane takes no clicks and no sideways scrolling.
        draw_wide(&mut app);
        let checks = app.drawn.areas.checks;
        assert!(checks.is_empty());
        press_at(&mut app, Position::new(checks.x, checks.y));
        assert_eq!(app.focus, Pane::Comments);
        press(&mut app, "0");
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Pane::Description);
    }

    #[test]
    fn hiding_the_diff_stops_its_fetches_until_it_is_shown_again() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "5");
        assert!(app.diffs_to_fetch().is_empty());
        press(&mut app, "5");
        assert_eq!(app.diffs_to_fetch().len(), 1);
    }

    #[test]
    fn a_hidden_panes_stale_data_is_dropped_so_nothing_acts_on_it() {
        let mut app = app_with_long_panes();
        draw_wide(&mut app);
        assert!(!app.drawn.comment_layout.items.is_empty());
        assert!(app.drawn.checks_layout.count > 0);
        press(&mut app, "1");
        press(&mut app, "2");
        press(&mut app, "3");
        draw_wide(&mut app);
        assert!(app.drawn.comment_layout.items.is_empty());
        assert_eq!(app.drawn.checks_layout.count, 0);
        assert!(app.drawn.description_links.is_empty());
        assert!(!app.drawn.areas.scrollbars.iter().any(|b| matches!(
            b.target,
            ScrollTarget::Checks | ScrollTarget::Comments | ScrollTarget::Description
        )));
    }

    /// The divider between the items `a` and `b` of `stack`, as last drawn.
    fn divider(app: &App, stack: StackId, a: usize, b: usize) -> Divider {
        app.drawn
            .areas
            .dividers
            .iter()
            .find(|d| d.stack == stack && d.between == (a, b))
            .unwrap_or_else(|| panic!("no divider {stack:?} {a} {b}"))
            .clone()
    }

    /// Drags the border of `divider` to `to` along its axis, as a mouse would, and draws.
    fn drag_divider(app: &mut App, divider: &Divider, to: u16, size: (u16, u16)) {
        // In the middle of the border, away from the corners, which belong to other things.
        let from = Position::new(
            divider.hit.x + divider.hit.width / 2,
            divider.hit.y + divider.hit.height / 2,
        );
        press_at(app, from);
        let target = if divider.stacked {
            Position::new(from.x + 3, to)
        } else {
            Position::new(to, from.y + 3)
        };
        drag_to(app, target);
        release_at(app, target);
        draw(app, size.0, size.1);
    }

    #[test]
    fn dragging_the_border_between_the_columns_resizes_them_and_clamps() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw_wide(&mut app);
        let columns = divider(&app, StackId::Columns, 0, 1);
        assert_eq!(app.drawn.areas.table.width, 100);
        drag_divider(&mut app, &columns, 60, (200, 40));
        assert_eq!(app.drawn.areas.table.width, 60);
        assert_eq!(app.drawn.areas.diff.x, 60);
        // As far as the pointer goes either way, the panes keep a minimum.
        let columns = divider(&app, StackId::Columns, 0, 1);
        drag_divider(&mut app, &columns, 0, (200, 40));
        assert_eq!(app.drawn.areas.table.width, MIN_COLUMNS);
        let columns = divider(&app, StackId::Columns, 0, 1);
        drag_divider(&mut app, &columns, 199, (200, 40));
        assert_eq!(app.drawn.areas.diff.width, MIN_COLUMNS);
    }

    #[test]
    fn dragging_a_border_inside_a_column_resizes_the_two_panes_beside_it() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw_wide(&mut app);
        let (list, checks, comments) = (
            app.drawn.areas.table,
            app.drawn.areas.checks,
            app.drawn.areas.comments,
        );
        let border = divider(&app, StackId::Left, 0, 1);
        // Down by five rows: the list grows and the checks lose what it gained.
        drag_divider(&mut app, &border, checks.y + 5, (200, 40));
        let after = (
            app.drawn.areas.table,
            app.drawn.areas.checks,
            app.drawn.areas.comments,
        );
        assert_eq!(after.0.height, list.height + 5);
        assert_eq!(after.1.height + 5, checks.height);
        // The comments, on the other side of the checks, were not touched.
        assert_eq!(after.2.height, comments.height);
        // The Description over the diff, on the right, is its own stack.
        let detail = divider(&app, StackId::Detail, 0, 1);
        let description = app.drawn.areas.description;
        drag_divider(&mut app, &detail, description.bottom() + 4, (200, 40));
        assert!(app.drawn.areas.description.height > description.height);
    }

    #[test]
    fn a_resized_layout_keeps_its_fractions_when_the_terminal_changes_size() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw_wide(&mut app);
        let columns = divider(&app, StackId::Columns, 0, 1);
        drag_divider(&mut app, &columns, 50, (200, 40));
        assert_eq!(app.drawn.areas.table.width, 50);
        // Twice as wide: still a quarter. And narrow enough for the other layout, the width
        // limit still decides which layout it is.
        draw(&mut app, 400, 40);
        assert_eq!(app.drawn.areas.table.width, 100);
        draw(&mut app, 120, 40);
        assert_eq!(
            app.drawn.areas.table.width, 120,
            "narrow: the list is on top"
        );
        assert!(app.drawn.areas.checks.y > app.drawn.areas.table.y);
    }

    #[test]
    fn the_narrow_layouts_borders_can_be_dragged_too() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw_narrow(&mut app);
        let list = app.drawn.areas.table;
        let rows = divider(&app, StackId::Rows, 0, 1);
        drag_divider(&mut app, &rows, list.bottom() + 6, (100, 40));
        assert_eq!(app.drawn.areas.table.height, list.height + 6);
        let sides = divider(&app, StackId::DetailColumns, 0, 1);
        let before = app.drawn.areas.description.width;
        drag_divider(&mut app, &sides, 30, (100, 40));
        assert!(app.drawn.areas.description.width < before);
        let lower = divider(&app, StackId::Lower, 0, 1);
        drag_divider(&mut app, &lower, 20, (100, 40));
        assert_eq!(app.drawn.areas.checks.width, 20);
    }

    #[test]
    fn a_click_on_a_border_does_not_resize() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw_wide(&mut app);
        let checks = app.drawn.areas.checks;
        press_at(&mut app, Position::new(checks.x + 4, checks.y));
        release_at(&mut app, Position::new(checks.x + 4, checks.y));
        assert!(app.sizes.is_default());
    }

    #[test]
    fn plus_and_minus_and_the_arrows_resize_the_focused_pane_and_equals_resets() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw_wide(&mut app);
        let list = app.drawn.areas.table;
        press(&mut app, "+");
        draw_wide(&mut app);
        assert!(app.drawn.areas.table.height > list.height);
        // A draw between the presses, as there is in use: the border is read from the last one.
        press(&mut app, "-");
        draw_wide(&mut app);
        press(&mut app, "-");
        draw_wide(&mut app);
        assert!(app.drawn.areas.table.height < list.height);
        // Wider and narrower, and from the details' side the other way round.
        press(&mut app, ">");
        draw_wide(&mut app);
        assert!(app.drawn.areas.table.width > list.width);
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Pane::Description);
        let width = app.drawn.areas.description.width;
        press(&mut app, ">");
        draw_wide(&mut app);
        assert!(app.drawn.areas.description.width > width);
        // The last pane of a stack has the border before it to move, and `=` puts it all back.
        press(&mut app, "=");
        draw_wide(&mut app);
        assert!(app.sizes.is_default());
        assert_eq!(app.drawn.areas.table, list);
    }

    #[test]
    fn a_pane_with_no_border_to_move_that_way_says_so() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        // Narrow: the list takes the whole width, so there is no wider or narrower for it.
        draw_narrow(&mut app);
        press(&mut app, ">");
        assert!(
            app.note
                .as_deref()
                .unwrap()
                .contains("can't be made wider or narrower")
        );
        assert!(app.sizes.is_default());
    }

    #[test]
    fn a_pane_hidden_when_the_stack_was_resized_does_not_come_back_squashed() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "1");
        draw_wide(&mut app);
        let border = divider(&app, StackId::Left, 0, 2);
        let top = app.drawn.areas.table;
        drag_divider(&mut app, &border, top.bottom() + 2, (200, 40));
        press(&mut app, "1");
        draw_wide(&mut app);
        let checks = app.drawn.areas.checks;
        assert!(checks.height >= MIN_ROWS, "{checks:?}");
    }

    #[test]
    fn a_click_on_a_vertical_track_pages_towards_it() {
        let mut app = app_with_long_panes();
        let hit = bar(&app, ScrollTarget::Diff);
        assert!(hit.vertical && hit.offset == 0);
        // Just above the bottom of the track, which is below the thumb.
        let below = Position::new(hit.track.x, hit.track.bottom() - 1);
        press_at(&mut app, below);
        release_at(&mut app, below);
        draw_wide(&mut app);
        let after = bar(&app, ScrollTarget::Diff);
        assert_eq!(after.offset, hit.viewport, "a page down");
        // And a click above the thumb pages back up to the start.
        let above = Position::new(hit.track.x, hit.track.y);
        press_at(&mut app, above);
        release_at(&mut app, above);
        draw_wide(&mut app);
        assert_eq!(bar(&app, ScrollTarget::Diff).offset, 0);
        // Nothing else happened to the click: the focus stays where it was.
        assert_eq!(app.focus, Pane::Table);
    }

    #[test]
    fn dragging_a_vertical_thumb_to_the_bottom_scrolls_to_the_end_even_off_the_bar() {
        let mut app = app_with_long_panes();
        let hit = bar(&app, ScrollTarget::Diff);
        // Take hold of the thumb at the top of the track, and pull it to the very bottom with the
        // pointer far to the left, off the bar.
        press_at(&mut app, Position::new(hit.track.x, hit.track.y));
        drag_to(&mut app, Position::new(2, hit.track.bottom() + 5));
        draw_wide(&mut app);
        let end = bar(&app, ScrollTarget::Diff);
        assert_eq!(end.offset, end.content - end.viewport, "to the end");
    }

    #[test]
    fn dragging_a_vertical_thumb_back_to_the_middle_scrolls_to_about_the_middle() {
        let mut app = app_with_long_panes();
        let hit = bar(&app, ScrollTarget::Diff);
        press_at(&mut app, Position::new(hit.track.x, hit.track.y));
        drag_to(&mut app, Position::new(2, hit.track.bottom() + 5));
        draw_wide(&mut app);
        let end = bar(&app, ScrollTarget::Diff);
        drag_to(
            &mut app,
            Position::new(2, hit.track.y + hit.track.height / 2),
        );
        draw_wide(&mut app);
        let mid = bar(&app, ScrollTarget::Diff).offset;
        let max = end.content - end.viewport;
        assert!(mid > max / 4 && mid < max * 3 / 4, "{mid} of {max}");
    }

    #[test]
    fn once_released_a_drag_does_nothing() {
        let mut app = app_with_long_panes();
        let hit = bar(&app, ScrollTarget::Diff);
        let middle = Position::new(2, hit.track.y + hit.track.height / 2);
        press_at(&mut app, Position::new(hit.track.x, hit.track.y));
        drag_to(&mut app, middle);
        draw_wide(&mut app);
        let mid = bar(&app, ScrollTarget::Diff).offset;
        assert!(mid > 0, "the drag scrolled");
        release_at(&mut app, middle);
        drag_to(&mut app, Position::new(2, hit.track.y));
        draw_wide(&mut app);
        assert_eq!(bar(&app, ScrollTarget::Diff).offset, mid);
    }
    #[test]
    fn dragging_the_lists_thumb_scrolls_the_list_and_keeps_the_highlight_on_screen() {
        let prs: Vec<Pr> = (1..=60).map(|n| by("alice", n, "Bump alpha")).collect();
        let mut app = app_with(prs);
        draw_wide(&mut app);
        let hit = bar(&app, ScrollTarget::Table);
        press_at(&mut app, Position::new(hit.track.x, hit.track.y));
        drag_to(&mut app, Position::new(hit.track.x, hit.track.bottom() - 1));
        draw_wide(&mut app);
        let after = bar(&app, ScrollTarget::Table);
        assert_eq!(after.offset, after.content - after.viewport);
        let selected = app.table.selected().unwrap();
        assert!(
            (after.offset..after.offset + after.viewport).contains(&selected),
            "{selected} is off screen"
        );
    }

    #[test]
    fn dragging_a_horizontal_thumb_scrolls_sideways() {
        let mut app = app_with_a_long_title();
        draw_narrow(&mut app);
        let hit = bar(&app, ScrollTarget::ListX);
        assert!(!hit.vertical);
        press_at(&mut app, Position::new(hit.track.x, hit.track.y));
        // Off the bar, below it, as far right as the track goes.
        drag_to(
            &mut app,
            Position::new(hit.track.right() + 3, hit.track.y + 4),
        );
        draw_narrow(&mut app);
        let end = bar(&app, ScrollTarget::ListX);
        assert_eq!(end.offset, end.content - end.viewport);
        assert_eq!(usize::from(app.list_x), end.offset);
    }

    #[test]
    fn a_click_on_the_horizontal_track_left_of_the_thumb_pages_back() {
        let mut app = app_with_a_long_title();
        draw_narrow(&mut app);
        let hit = bar(&app, ScrollTarget::ListX);
        press_at(&mut app, Position::new(hit.track.x, hit.track.y));
        drag_to(&mut app, Position::new(hit.track.right() + 3, hit.track.y));
        release_at(&mut app, Position::new(0, 0));
        draw_narrow(&mut app);
        let end = bar(&app, ScrollTarget::ListX);
        press_at(&mut app, Position::new(hit.track.x, hit.track.y));
        release_at(&mut app, Position::new(hit.track.x, hit.track.y));
        draw_narrow(&mut app);
        assert!(bar(&app, ScrollTarget::ListX).offset < end.offset);
    }
    #[test]
    fn a_bar_that_is_not_drawn_has_nothing_to_click() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw_wide(&mut app);
        assert!(
            !app.drawn
                .areas
                .scrollbars
                .iter()
                .any(|h| h.target == ScrollTarget::Table)
        );
        // A click on the table's right border focuses it as before.
        let table = app.drawn.areas.table;
        press_at(&mut app, Position::new(table.right() - 1, table.y + 3));
        assert_eq!(app.focus, Pane::Table);
    }

    #[test]
    fn the_wheel_scrolls_the_pane_under_the_pointer_without_moving_focus() {
        let mut app = app_with_long_panes();
        let inside = |area: Rect| Position::new(area.x + 2, area.y + 1);
        let areas = &app.drawn.areas;
        let at = Points {
            table: inside(areas.table),
            checks: inside(areas.checks),
            description: inside(areas.description),
            diff: inside(areas.diff),
            comments: inside(areas.comments),
        };

        wheel(&mut app, at.table, true);
        assert_eq!(app.table.selected(), Some(1));
        wheel(&mut app, at.table, false);
        assert_eq!(app.table.selected(), Some(0));

        wheel(&mut app, at.checks, true);
        assert_eq!(app.scroll.check, 1);
        wheel(&mut app, at.description, true);
        assert_eq!(app.scroll.description, 3);
        wheel(&mut app, at.diff, true);
        wheel(&mut app, at.diff, true);
        assert_eq!(app.scroll.diff, 6);
        wheel(&mut app, at.diff, false);
        assert_eq!(app.scroll.diff, 3);

        // Over the comments it moves the highlight to the next comment, as Down does.
        wheel(&mut app, at.comments, true);
        assert_eq!(app.scroll.comment, 1);
        assert_eq!(app.scroll.comments, 0);

        assert_eq!(app.focus, Pane::Table);
    }

    #[test]
    fn a_drag_does_nothing() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
        ]);
        draw_wide(&mut app);
        let rows = app.drawn.areas.table_rows;
        let diff = app.drawn.areas.diff;
        let drag = |app: &mut App, x: u16, y: u16| drag_to(app, Position::new(x, y));
        // Over a row's checkbox, a pane, a box, and then in a box being edited and a dialog.
        drag(&mut app, rows.x + 1, rows.y + 1);
        drag(&mut app, diff.x + 2, diff.y + 1);
        let search = app.drawn.areas.search;
        drag(&mut app, search.x + 2, search.y + 1);
        press(&mut app, "/");
        drag(&mut app, diff.x + 2, diff.y + 1);
        key(&mut app, KeyCode::Esc);
        press(&mut app, "?");
        drag(&mut app, diff.x + 2, diff.y + 1);

        assert!(matches!(app.mode, Mode::Help(_)));
        assert!(app.selected.is_empty());
        assert_eq!(app.highlighted().unwrap().pr.number, 1);
        assert_eq!(app.focus, Pane::Table);
    }

    #[test]
    fn a_wide_terminal_puts_the_list_beside_the_details() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw_wide(&mut app);
        let areas = &app.drawn.areas;
        // The list, checks and comments are one column on the left, the details on the right.
        assert_eq!(areas.table.x, 0);
        assert!(areas.table.width <= 100);
        assert_eq!(areas.checks.y, areas.table.bottom());
        assert_eq!(areas.comments.y, areas.checks.bottom());
        assert!(areas.description.x >= areas.table.right());
        assert_eq!(areas.diff.x, areas.description.x);
    }

    #[test]
    fn a_narrow_terminal_puts_the_description_beside_the_diff_above_the_checks_and_comments() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 120, 40);
        let areas = &app.drawn.areas;
        // The list has the full width, above everything else.
        assert_eq!((areas.table.x, areas.table.width), (0, 120));
        for (name, area) in [
            ("checks", areas.checks),
            ("comments", areas.comments),
            ("description", areas.description),
            ("diff", areas.diff),
        ] {
            assert!(area.y >= areas.table.bottom(), "{name} is below the list");
        }
        // The description is on the left and the diff on the right, in the same rows.
        assert_eq!(areas.description.x, 0);
        assert!(areas.diff.x >= areas.description.right());
        assert_eq!(
            areas.approvers.y,
            areas.description.y.saturating_sub(PULL_REQUEST_ROWS)
        );
        assert_eq!(areas.diff.y, areas.approvers.bottom());
        assert_eq!(areas.diff.bottom(), areas.description.bottom());
        // The checks and comments are side by side below them, a short row.
        assert_eq!(areas.checks.y, areas.description.bottom());
        assert_eq!(areas.comments.y, areas.checks.y);
        assert_eq!(areas.checks.x, 0);
        assert!(areas.comments.x >= areas.checks.right());
        assert_eq!(areas.comments.bottom(), 40 - 1);
        assert!(areas.checks.height <= MAX_COMMENTS_ROWS);
        assert!(areas.description.height > areas.checks.height);
    }

    #[test]
    fn a_short_narrow_terminal_still_gives_the_checks_and_comments_a_few_rows() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 100, 24);
        assert!(app.drawn.areas.checks.height >= MIN_LOWER_ROWS);
        assert!(app.drawn.areas.comments.height >= MIN_LOWER_ROWS);
    }

    #[test]
    fn the_layout_changes_at_the_width_limit() {
        for (width, wide) in [
            (WIDE_LAYOUT_MIN_WIDTH - 1, false),
            (WIDE_LAYOUT_MIN_WIDTH, true),
        ] {
            let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
            draw(&mut app, width, 40);
            let areas = &app.drawn.areas;
            if wide {
                assert!(areas.description.x >= areas.table.right(), "{width}");
            } else {
                assert_eq!(areas.table.width, width, "{width}");
            }
        }
    }

    /// The rectangles of the panes `layout` gives, with their names.
    fn rects(layout: &PaneLayout) -> [(&'static str, Rect); 7] {
        [
            ("list", layout.list),
            ("checks", layout.checks),
            ("approvers", layout.approvers),
            ("comments", layout.comments),
            ("pull_request", layout.pull_request),
            ("description", layout.description),
            ("diff", layout.diff),
        ]
    }

    /// Every set of hidden panes, the table aside (it is always shown).
    fn every_hidden_set() -> Vec<Vec<Pane>> {
        let panes = [
            Pane::Checks,
            Pane::Comments,
            Pane::Description,
            Pane::Approvers,
            Pane::Diff,
        ];
        (0..1 << panes.len())
            .map(|bits| {
                (0..panes.len())
                    .filter(|i| bits & (1 << i) != 0)
                    .map(|i| panes[i])
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_shown_panes_stay_inside_the_body_and_never_overlap() {
        for width in [
            50,
            100,
            WIDE_LAYOUT_MIN_WIDTH - 1,
            WIDE_LAYOUT_MIN_WIDTH,
            300,
        ] {
            let body = Rect::new(0, 0, width, 40);
            for hidden in every_hidden_set() {
                let layout = layout(width, 40, &hidden, &Sizes::default());
                let shown: Vec<_> = rects(&layout)
                    .into_iter()
                    .filter(|(_, rect)| !rect.is_empty())
                    .collect();
                for (name, rect) in &shown {
                    assert!(
                        body.union(*rect) == body,
                        "{name} {rect:?} is outside the body at {width} hiding {hidden:?}"
                    );
                }
                for (i, (a_name, a)) in shown.iter().enumerate() {
                    for (b_name, b) in &shown[i + 1..] {
                        assert!(
                            !a.intersects(*b),
                            "{a_name} {a:?} and {b_name} {b:?} overlap at {width} hiding {hidden:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_hidden_pane_has_an_empty_rectangle() {
        for width in [100, 200] {
            for hidden in every_hidden_set() {
                let layout = layout(width, 40, &hidden, &Sizes::default());
                for (pane, rect) in [
                    (Pane::Checks, layout.checks),
                    (Pane::Comments, layout.comments),
                    (Pane::Description, layout.description),
                    (Pane::Approvers, layout.approvers),
                    (Pane::Diff, layout.diff),
                ] {
                    assert_eq!(
                        hidden.contains(&pane),
                        rect.is_empty(),
                        "{pane:?} {hidden:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_tiny_body_does_not_panic() {
        for width in [20, WIDE_LAYOUT_MIN_WIDTH] {
            for height in 0..=8 {
                for hidden in every_hidden_set() {
                    layout(width, height, &hidden, &Sizes::default());
                }
            }
        }
    }

    #[test]
    fn the_approvers_take_the_diffs_room_when_the_diff_is_hidden() {
        let layout = layout(200, 40, &[Pane::Diff], &Sizes::default());
        assert!(layout.diff.is_empty());
        assert!(
            layout.approvers.height > MAX_APPROVERS_ROWS,
            "{:?}",
            layout.approvers
        );
        // With the diff shown they have only the rows they asked for.
        let layout = self::layout(200, 40, &[], &Sizes::default());
        assert_eq!(layout.approvers.height, 3);
    }

    #[test]
    fn the_approvers_are_at_most_a_third_of_their_room() {
        // Room for 5 rows above and below, so a third is 1, under the 3 asked for.
        let layout = layout(200, 12, &[Pane::Checks, Pane::Comments], &Sizes::default());
        let room = layout.approvers.height + layout.diff.height;
        assert!(room < 9, "{room}");
        assert_eq!(layout.approvers.height, room / 3);
    }

    #[test]
    fn the_pull_request_box_is_over_the_diff_when_the_description_is_hidden() {
        let layout = layout(100, 40, &[Pane::Description], &Sizes::default());
        assert!(layout.description.is_empty());
        let pull_request = layout.pull_request;
        assert_eq!(pull_request.height, PULL_REQUEST_ROWS);
        assert_eq!(pull_request.bottom(), layout.approvers.y);
        assert_eq!(pull_request.x, layout.approvers.x);
        assert_eq!(pull_request.width, layout.diff.width);
    }

    #[test]
    fn the_pull_request_box_goes_with_the_details() {
        let details = [Pane::Description, Pane::Approvers, Pane::Diff];
        for width in [100, 200] {
            let layout = layout(width, 40, &details, &Sizes::default());
            assert!(layout.pull_request.is_empty(), "{width}");
            // The list has the whole width.
            assert_eq!(layout.list.width, width, "{width}");
        }
    }

    /// The divider of `stack` between its items `a` and `b`.
    fn divider_of(layout: &PaneLayout, stack: StackId, a: usize, b: usize) -> Divider {
        layout
            .dividers
            .iter()
            .find(|d| d.stack == stack && d.between == (a, b))
            .unwrap_or_else(|| panic!("no divider {stack:?} {a} {b}"))
            .clone()
    }

    #[test]
    fn a_divider_between_stacked_panes_sits_on_their_borders() {
        let layout = layout(200, 40, &[], &Sizes::default());
        let (top, below) = (layout.list, layout.checks);
        let divider = divider_of(&layout, StackId::Left, 0, 1);
        assert!(divider.stacked);
        assert_eq!(
            divider.hit,
            Rect::new(top.x, top.bottom() - 1, top.width, 2)
        );
        assert_eq!(divider.boundary, below.y);
        assert_eq!(divider.start, top.y);
        assert_eq!(divider.end, below.bottom());
        assert_eq!(divider.before, [Pane::Table]);
        assert_eq!(divider.after, [Pane::Checks]);
    }

    #[test]
    fn a_divider_between_side_by_side_panes_sits_on_their_borders() {
        let layout = layout(200, 40, &[], &Sizes::default());
        let (left, right) = (layout.list, layout.description);
        let divider = divider_of(&layout, StackId::Columns, 0, 1);
        assert!(!divider.stacked);
        assert_eq!(divider.hit.x, left.right() - 1);
        assert_eq!(divider.hit.width, 2);
        assert_eq!(divider.boundary, right.x);
        assert_eq!(divider.start, 0);
        assert_eq!(divider.end, 200);
    }

    #[test]
    fn a_divider_skips_a_hidden_pane() {
        let layout = layout(200, 40, &[Pane::Checks], &Sizes::default());
        assert!(
            layout
                .dividers
                .iter()
                .all(|d| d.between != (0, 1) || d.stack != StackId::Left)
        );
        let divider = divider_of(&layout, StackId::Left, 0, 2);
        assert_eq!(divider.boundary, layout.comments.y);
        assert_eq!(
            divider.items.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [0, 2]
        );
    }

    #[test]
    fn narrow_dividers_are_between_the_rows_and_columns() {
        let layout = layout(100, 40, &[], &Sizes::default());
        let rows = divider_of(&layout, StackId::Rows, 0, 1);
        assert_eq!(rows.boundary, layout.pull_request.y);
        let columns = divider_of(&layout, StackId::DetailColumns, 0, 1);
        assert_eq!(columns.boundary, layout.approvers.x);
        let lower = divider_of(&layout, StackId::Lower, 0, 1);
        assert_eq!(lower.boundary, layout.comments.x);
        assert_eq!(lower.before, [Pane::Checks]);
        assert_eq!(lower.after, [Pane::Comments]);
    }

    #[test]
    fn a_resized_stack_shares_its_room_by_its_weights() {
        for (width, stack, size) in [
            (200, StackId::Columns, (200, 40)),
            (100, StackId::Rows, (100, 40)),
        ] {
            let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
            draw(&mut app, size.0, size.1);
            let before = layout(width, 39, &[], &app.sizes);
            let divider = divider_of(&before, stack, 0, 1);
            let to = if divider.stacked {
                divider.boundary + 4
            } else {
                divider.boundary + 20
            };
            drag_divider(&mut app, &divider, to, size);
            assert!(!app.sizes.is_default(), "{stack:?}");
            let after = layout(width, 39, &[], &app.sizes);
            let moved = divider_of(&after, stack, 0, 1);
            assert!(moved.boundary > divider.boundary, "{stack:?}");
            assert_eq!(moved.start, divider.start);
            assert_eq!(moved.end, divider.end);
        }
    }

    #[test]
    fn the_wheel_scrolls_the_list_and_the_diff_sideways() {
        use crossterm::event::{KeyModifiers, MouseEventKind};
        let mut app = app_with_a_long_title();
        app.diff_loaded(1, Ok(format!("+{}END", "x".repeat(300))));
        draw_narrow(&mut app);
        let table = app.drawn.areas.table;
        let diff = app.drawn.areas.diff;
        let scroll = |app: &mut App, kind, modifiers, area: Rect| {
            scroll_at(app, kind, modifiers, Position::new(area.x + 2, area.y + 2));
        };
        scroll(
            &mut app,
            MouseEventKind::ScrollRight,
            KeyModifiers::NONE,
            table,
        );
        assert!(app.list_x > 0);
        scroll(
            &mut app,
            MouseEventKind::ScrollDown,
            KeyModifiers::SHIFT,
            diff,
        );
        assert!(app.scroll.diff_x > 0);
        // The focus stays where it was.
        assert_eq!(app.focus, Pane::Table);
    }

    #[test]
    fn the_wheel_is_ignored_while_a_box_is_edited() {
        let mut app = app_with_long_panes();
        draw_wide(&mut app);
        let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
        press(&mut app, "f");
        wheel(&mut app, diff, true);
        assert_eq!(app.scroll.diff, 0);
    }

    #[test]
    fn the_approvers_pane_is_above_the_diff_in_both_layouts() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        for width in [200, 100] {
            draw(&mut app, width, 40);
            let areas = &app.drawn.areas;
            let (approvers, description, diff) = (areas.approvers, areas.description, areas.diff);
            // The diff is right under it, in the same columns.
            assert_eq!((approvers.x, approvers.width), (diff.x, diff.width));
            assert_eq!(approvers.bottom(), diff.y);
            // Wide, the description is above it, in the same column. Narrow, it is beside it.
            if width == 200 {
                assert_eq!(approvers.y, description.bottom());
                assert_eq!(approvers.x, description.x);
            } else {
                assert!(approvers.x >= description.right());
            }
            // With a line to say, it is as short as it can be.
            assert_eq!(approvers.height, MIN_APPROVERS_ROWS);
        }
    }
}
