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
    self, App, ChecksLayout, CommentLayout, ConfirmOpen, Details, Fetch, Mode, Page, Pane,
    PopupView, Typing, to_u16,
};
use crate::batch::{Batch, Confirm, ItemStatus, Summary};
use crate::gh::{Approvers, Comment, PrState, Reviewer, Verdict};
use crate::keys;
use crate::model::{self, CheckState, Column, Sort};

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

/// The widest a popup gets, however wide the terminal is.
const MAX_POPUP_WIDTH: u16 = 90;

/// The widths of the table's columns other than the title. Each fits its longest value, such as
/// `#12345`, `· no checks` and `✓ approved`, and a space.
const NUMBER_WIDTH: u16 = 7;
const CHECKS_WIDTH: u16 = 12;
const REVIEW_WIDTH: u16 = 11;
/// Fits `app/renovate` and a typical login; a longer one is cut.
const AUTHOR_WIDTH: u16 = 16;
/// Fits the header with its arrow, `Updated ▲`; a cell is an age such as `3d`.
const UPDATED_WIDTH: u16 = 9;

/// The characters of a horizontal scrollbar. The track is the border's own, so only the thumb
/// shows.
const HORIZONTAL_THUMB: &str = "▬";
const HORIZONTAL_TRACK: &str = "─";

/// The columns of the table that stay in view when it scrolls sideways: the checkbox and the
/// number, and the gap after each.
const FROZEN_WIDTH: u16 = CHECKBOX_WIDTH + 1 + NUMBER_WIDTH + 1;

/// The most columns a title column gets when the table scrolls, so one huge title can't make the
/// table enormous.
const MAX_TITLE_COLUMN_WIDTH: usize = 300;

/// The columns between two buttons.
const BUTTON_GAP: u16 = 2;

/// The width of the checks pane's status column, which fits `… pending` and a space.
const CHECK_STATUS_WIDTH: u16 = 10;

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
}

/// The approvers over the diff, in `area`. The approvers get `wanted` rows, but never more than a
/// third of the area.
fn approvers_over_diff(area: Rect, wanted: u16) -> [Rect; 2] {
    let rows = wanted.min(area.height / 3);
    Layout::vertical([Constraint::Length(rows), Constraint::Fill(1)]).areas(area)
}

/// With room, the list is above the checks and comments on the left and the PR's details are on
/// the right. In a narrow terminal that would leave the table too little width, so the list takes
/// the full width on top. Under it, what is read most gets the space: the description beside the
/// diff, with the checks and comments in a short row at the bottom. The approvers are in the
/// details, above the diff, and take `approvers_rows` if they can.
fn pane_layout(body: Rect, approvers_rows: u16) -> PaneLayout {
    let halves = |area| Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);
    if body.width >= WIDE_LAYOUT_MIN_WIDTH {
        let [left, detail] = halves(body);
        let [list, checks, comments] = Layout::vertical([
            Constraint::Fill(2),
            Constraint::Fill(1),
            Constraint::Max(MAX_COMMENTS_ROWS),
        ])
        .areas(left);
        let [pull_request, description, diff_pane] = Layout::vertical([
            Constraint::Length(PULL_REQUEST_ROWS),
            Constraint::Fill(1),
            Constraint::Fill(2),
        ])
        .areas(detail);
        let [approvers, diff] = approvers_over_diff(diff_pane, approvers_rows);
        PaneLayout {
            list,
            checks,
            approvers,
            comments,
            pull_request,
            description,
            diff,
        }
    } else {
        let lower_rows = (body.height / 5).clamp(MIN_LOWER_ROWS, MAX_COMMENTS_ROWS);
        let [list, details, lower] = Layout::vertical([
            Constraint::Fill(2),
            Constraint::Fill(3),
            Constraint::Length(lower_rows),
        ])
        .areas(body);
        let [left, diff_pane] = halves(details);
        let [pull_request, description] =
            Layout::vertical([Constraint::Length(PULL_REQUEST_ROWS), Constraint::Fill(1)])
                .areas(left);
        let [approvers, diff] = approvers_over_diff(diff_pane, approvers_rows);
        let [checks, comments] = halves(lower);
        PaneLayout {
            list,
            checks,
            approvers,
            comments,
            pull_request,
            description,
            diff,
        }
    }
}

pub fn view(app: &mut App, frame: &mut Frame) {
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
    } = pane_layout(body_area, approvers_rows(app, body_area.width / 2));
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
    app.drawn.areas.filter = if filter_rows > 0 {
        filter_area
    } else {
        Rect::default()
    };
    app.drawn.areas.columns = if columns_rows > 0 {
        columns_area
    } else {
        Rect::default()
    };
    search(app, frame, search_area);
    if filter_rows > 0 {
        filter(app, frame, filter_area);
    }
    if columns_rows > 0 {
        columns(app, frame, columns_area);
    }

    pr_table(app, frame, table_area);
    checks(app, frame, checks_area);
    approvers(app, frame, approvers_area);
    comments(app, frame, comments_area);
    pull_request(app, frame, pull_request_area);
    description(app, frame, description_area);
    diff(app, frame, diff_area);

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

/// The popup over the table that the mode asks for, if any. Records where its buttons are.
fn draw_dialog(app: &mut App, frame: &mut Frame, area: Rect) {
    let mut scrolled = None;
    let buttons = match &app.mode {
        Mode::Confirm(confirm) => confirm_dialog(confirm, app.dry_run, frame, area),
        Mode::ConfirmOpen(confirm) => open_dialog(confirm, frame, area),
        Mode::Help(offset) => {
            let (buttons, view) = help_popup(*offset, frame, area);
            scrolled = Some(view);
            buttons
        }
        Mode::PickColumns(at) => columns_popup(app, *at, frame, area),
        Mode::Running(batch) => running_popup(batch, app.dry_run, spinner(app), frame, area),
        Mode::Results(summary) => results_popup(summary, app.dry_run, frame, area),
        Mode::Loading | Mode::List => Vec::new(),
    };
    app.drawn.buttons = buttons;
    // Scrolled too far (the terminal grew, or End was pressed), it is brought back.
    app.drawn.popup = scrolled.unwrap_or_default();
    if let (Mode::Help(offset), Some(view)) = (&mut app.mode, scrolled) {
        *offset = view.offset;
    }
}

/// The hints of the modes other than the list, for the end of the status line. The list's are
/// built by `fit_status_line`, which has to fit them to the width.
fn mode_hints(app: &App) -> Option<&'static str> {
    match &app.mode {
        Mode::Confirm(_) | Mode::ConfirmOpen(_) => Some(keys::CHOOSE_HINTS),
        Mode::Help(_) => Some(keys::HELP_HINTS),
        Mode::PickColumns(_) => Some(keys::PICK_COLUMNS_HINTS),
        Mode::Running(_) => Some(keys::RUNNING_HINTS),
        Mode::Results(_) => Some(keys::RESULTS_HINTS),
        Mode::Loading | Mode::List => match app.typing {
            Typing::Search => Some(match app.completion() {
                Some((_, Some(_))) => keys::CHOSEN_HINTS,
                Some((_, None)) => keys::SUGGESTIONS_HINTS,
                None => keys::SEARCH_HINTS,
            }),
            Typing::Filter => Some(keys::FILTER_HINTS),
            Typing::Columns => Some(keys::COLUMNS_HINTS),
            Typing::Nothing => None,
        },
    }
}

/// The status bar: what is going on, then the keys that do something now, then the page `o`
/// opens, in `width` columns.
fn status_line(app: &App, width: u16) -> String {
    let mut parts = Vec::new();
    if app.list.searching {
        parts.push(format!("{} Searching", spinner(app)));
    }
    if app.refreshing() {
        parts.push(format!("{} Refreshing", spinner(app)));
    }
    if !app.selected.is_empty() {
        let hidden = app.hidden_selected();
        let mut selected = format!("{} selected", app.selected.len());
        if hidden > 0 {
            selected.push_str(&format!(" ({hidden} hidden)"));
        }
        parts.push(selected);
    }
    if let Some(failure) = &app.status.failure {
        parts.push(format!("Refresh failed: {failure}"));
    }
    if app.status.paused() {
        parts.push("Refresh paused".to_string());
    }
    parts.extend(app.note.clone());
    if let Some(hints) = mode_hints(app) {
        parts.push(hints.to_string());
        return parts.join(" | ");
    }
    let mut hints = keys::bar_hints(app.focus);
    let page = match (app.focus, app.highlighted_page()) {
        (_, Some(Page::Url(url))) => Some(url),
        // With no page there is no `o` hint either: it would promise one.
        (focus, Some(Page::Missing)) => {
            hints.retain(|hint| !hint.for_page);
            Some(if focus == Pane::Comments {
                "No page, o opens the PR".to_string()
            } else {
                "No page".to_string()
            })
        }
        (_, None) => None,
    };
    fit_status_line(parts, page, hints, usize::from(width))
}

/// The shortest the URL is cut to before the hints that aren't essential go.
const MIN_URL_WIDTH: usize = 24;

/// Joins the status `parts`, the `hints` and the `page` (a URL, or a note that there is none) in
/// `width` columns, the page last so that it takes the space the keys leave. A URL too long for it
/// is cut in the middle first; if it is still too long, the
/// hints that aren't essential go from the end, and the URL is cut as short as it must be.
fn fit_status_line(
    mut parts: Vec<String>,
    page: Option<String>,
    mut hints: Vec<keys::Hint>,
    width: usize,
) -> String {
    let join = |parts: &[String], page: &Option<String>, hints: &[keys::Hint]| {
        let mut all: Vec<&str> = parts.iter().map(String::as_str).collect();
        all.extend(hints.iter().map(|hint| hint.text.as_str()));
        all.extend(page.as_deref());
        all.join(" | ")
    };
    let Some(page) = page else {
        parts.extend(hints.into_iter().map(|hint| hint.text));
        return parts.join(" | ");
    };
    let width_of = |text: &str| unicode_width::UnicodeWidthStr::width(text);
    // What the line takes up apart from the page, with its separator.
    let others = |hints: &[keys::Hint]| width_of(&join(&parts, &None, hints)) + 3;
    let room = |hints: &[keys::Hint]| width.saturating_sub(others(hints));
    while room(&hints) < MIN_URL_WIDTH.min(width_of(&page)) {
        let Some(last) = hints.iter().rposition(|hint| !hint.essential) else {
            break;
        };
        hints.remove(last);
    }
    let page = middle_cut(&page, room(&hints));
    join(&parts, &Some(page), &hints)
}

/// `text` cut in the middle with `…` to fit `width` columns.
fn middle_cut(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let kept = width.saturating_sub(1);
    let head = kept.div_ceil(2);
    let tail = kept - head;
    let head: String = chars[..head].iter().collect();
    let tail: String = chars[chars.len() - tail..].iter().collect();
    format!("{head}…{tail}")
}

/// The width of the table's first column, which holds `[ ]` or `[x]`.
const CHECKBOX_WIDTH: u16 = 3;

/// The table of PRs in a bordered pane, or a message in its place while there are none.
fn pr_table(app: &mut App, frame: &mut Frame, area: Rect) {
    let table_block = pane_block(table_title(app), app.focus == Pane::Table);
    app.drawn.areas.table = area;
    app.drawn.areas.table_rows = Rect::default();
    app.drawn.areas.table_checkboxes = Rect::default();
    app.drawn.areas.table_header = Vec::new();
    let table_area = table_block.inner(area);
    frame.render_widget(table_block, area);
    // One row of the table is its header.
    set_page(app, Pane::Table, table_area.height, 2);

    match app.mode {
        Mode::Loading => frame.render_widget(Line::from("Loading…"), table_area),
        Mode::List if !app.rows.is_empty() && app.visible.is_empty() => {
            frame.render_widget(Line::from("No PRs match the filter."), table_area)
        }
        Mode::List if app.rows.is_empty() => {
            let message = if app.errors.text().is_some() {
                "No PRs loaded."
            } else {
                "No matching PRs found."
            };
            frame.render_widget(Line::from(message), table_area)
        }
        // Popups are drawn over the table.
        Mode::List
        | Mode::Confirm(_)
        | Mode::ConfirmOpen(_)
        | Mode::Help(_)
        | Mode::PickColumns(_)
        | Mode::Running(_)
        | Mode::Results(_) => {
            let shown = app.shown_columns();
            // The groups of the column regex share the space the title had.
            let mut titles = match &app.columns {
                Some(regex) => model::column_names(regex),
                None => vec!["Title".to_string()],
            };
            // The sorted column's header has an arrow: up for smallest first. A title sort goes by
            // the title columns in order, so it is marked on the first.
            let (sorted, descending) = app.sort();
            let arrow = if descending { " ▼" } else { " ▲" };
            let marked = |column: Sort, name: &str| {
                if sorted == column {
                    format!("{name}{arrow}")
                } else {
                    name.to_string()
                }
            };
            titles[0] = marked(Sort::Title, &titles[0]);
            // With more than fits, the title columns take the width of their longest cell and the
            // table scrolls sideways; otherwise they share what is left.
            let title_widths = title_column_widths(app, &titles);
            let fixed = CHECKBOX_WIDTH + shown.iter().map(|&c| column_width(c)).sum::<u16>();
            let gaps = (1 + shown.len() + titles.len() - 1) as u16;
            let total = fixed + title_widths.iter().sum::<u16>() + gaps;
            let scrolling = total > table_area.width;
            // Borrowing the fields rather than calling `visible_rows` leaves `app.table` free.
            let now = jiff::Timestamp::now();
            let rows = app.visible.iter().map(|&i| &app.rows[i]).map(|row| {
                let selected = app.selected.contains(&row.pr.number);
                pr_row(row, selected, &shown, app.columns.as_ref(), now)
            });
            let widths = [Constraint::Length(CHECKBOX_WIDTH)]
                .into_iter()
                .chain(
                    shown
                        .iter()
                        .map(|&column| Constraint::Length(column_width(column))),
                )
                .chain(title_widths.iter().map(|&width| {
                    if scrolling {
                        Constraint::Length(width)
                    } else {
                        Constraint::Fill(1)
                    }
                }))
                .collect::<Vec<_>>();
            let header = [String::new()]
                .into_iter()
                .chain(shown.iter().map(|&c| marked(c.sort(), c.name())))
                .chain(titles);
            let mut table = Table::new(rows, widths)
                .header(Row::new(header).bold())
                .row_highlight_style(Style::new().reversed());
            // The rows are about to be replaced.
            if app.list.searching {
                table = table.dim();
            }
            if scrolling {
                // Drawn at its full width, then the part in view is copied over, keeping the
                // first columns in place.
                let full = Rect::new(0, 0, total, table_area.height);
                let mut buffer = Buffer::empty(full);
                StatefulWidget::render(table, full, &mut buffer, &mut app.table);
                let x = app.list_x.min(total - table_area.width);
                app.list_x = x;
                copy_scrolled(
                    &buffer,
                    frame.buffer_mut(),
                    table_area,
                    frozen_width(&shown),
                    x,
                );
                hscrollbar(
                    frame,
                    area,
                    usize::from(total),
                    usize::from(table_area.width),
                    usize::from(x),
                );
            } else {
                app.list_x = 0;
                frame.render_stateful_widget(table, table_area, &mut app.table);
            }
            // One row of the table is its header.
            scrollbar(
                frame,
                area,
                app.visible.len(),
                usize::from(table_area.height.saturating_sub(1)),
                app.table.offset(),
            );
            // The header is one row.
            app.drawn.areas.table_rows = Rect {
                y: table_area.y + 1,
                height: table_area.height.saturating_sub(1),
                ..table_area
            };
            app.drawn.areas.table_checkboxes = Rect {
                width: CHECKBOX_WIDTH,
                ..app.drawn.areas.table_rows
            };
            app.drawn.areas.table_offset = app.table.offset();
            let full_width = if scrolling { total } else { table_area.width };
            app.drawn.areas.table_header = header_hits(
                table_area,
                &shown,
                full_width,
                frozen_width(&shown),
                app.list_x,
            );
        }
    }
}

/// Where each sortable column's header is on screen, for a click: each of the `shown` columns, and
/// one area for all the title columns, to the end of the table. The table is `full_width` columns
/// wide, scrolled `x` columns past its `frozen` ones, as drawn above. The checkbox has nothing to
/// sort by.
fn header_hits(
    area: Rect,
    shown: &[Column],
    full_width: u16,
    frozen: u16,
    x: u16,
) -> Vec<(Rect, Sort)> {
    // The columns, in the table's own coordinates, with the one column between each.
    let mut start = CHECKBOX_WIDTH + 1;
    let mut columns = Vec::new();
    for &column in shown {
        let end = start + column_width(column);
        columns.push((start, end, column.sort()));
        start = end + 1;
    }
    columns.push((start, full_width, Sort::Title));
    columns
        .into_iter()
        .filter_map(|(start, end, sort)| {
            // The frozen columns stay; the rest move with the scroll, and can't go under them.
            let (start, end) = if end <= frozen {
                (start, end)
            } else {
                (start.saturating_sub(x).max(frozen), end.saturating_sub(x))
            };
            let end = end.min(area.width);
            (start < end).then(|| (Rect::new(area.x + start, area.y, end - start, 1), sort))
        })
        .collect()
}

/// The width of the table's `column`.
fn column_width(column: Column) -> u16 {
    match column {
        Column::Number => NUMBER_WIDTH,
        Column::Checks => CHECKS_WIDTH,
        Column::Review => REVIEW_WIDTH,
        Column::Author => AUTHOR_WIDTH,
        Column::Updated => UPDATED_WIDTH,
    }
}

/// The width of the columns that stay in view when the table scrolls sideways: the checkbox, and
/// the number if it is shown, with the gap after each.
fn frozen_width(shown: &[Column]) -> u16 {
    if shown.contains(&Column::Number) {
        FROZEN_WIDTH
    } else {
        CHECKBOX_WIDTH + 1
    }
}

/// The widest cell of each title column, the header included, as far as is worth showing.
fn title_column_widths(app: &App, titles: &[String]) -> Vec<u16> {
    let mut widths: Vec<usize> = titles.iter().map(|title| title.width()).collect();
    for row in app.visible_rows() {
        for (width, cell) in widths.iter_mut().zip(row.title_cells(app.columns.as_ref())) {
            *width = (*width).max(cell.width());
        }
    }
    widths
        .into_iter()
        .map(|width| to_u16(width.min(MAX_TITLE_COLUMN_WIDTH)))
        .collect()
}

/// Copies `source`, which was drawn at its full width from the top left, into `area` of `target`,
/// scrolled `x` columns past its first `frozen` columns, which stay.
fn copy_scrolled(source: &Buffer, target: &mut Buffer, area: Rect, frozen: u16, x: u16) {
    for dy in 0..area.height {
        for dx in 0..area.width {
            let from = if dx < frozen { dx } else { dx + x };
            target[(area.x + dx, area.y + dy)] = source[(from, dy)].clone();
        }
    }
}

/// A PR's row of the table, with a cell for each of the `shown` columns. A closed or merged PR is
/// dimmed.
fn pr_row<'a>(
    row: &'a app::Row,
    selected: bool,
    shown: &[Column],
    columns: Option<&Regex>,
    now: jiff::Timestamp,
) -> Row<'a> {
    let mark = if selected { "[x]" } else { "[ ]" };
    let cell = |column: Column| {
        let cell = Cell::new(row.cell(column, now));
        if column == Column::Checks {
            cell.style(checks_style(row))
        } else {
            cell
        }
    };
    let cells = Row::new(
        [Cell::new(mark)]
            .into_iter()
            .chain(shown.iter().map(|&column| cell(column)))
            .chain(row.title_cells(columns).into_iter().map(Cell::new)),
    );
    if row.pr.state == PrState::Open {
        cells
    } else {
        cells.dim()
    }
}

/// The popup that chooses the table's columns: a checkbox for each, and the one at `at`
/// highlighted. The sorted column is named, as hiding it leaves the sort in place.
fn columns_popup(app: &App, at: usize, frame: &mut Frame, area: Rect) -> Vec<Rect> {
    let lines = Column::ALL
        .into_iter()
        .enumerate()
        .map(|(i, column)| {
            // The author also has no column when every PR has the same one.
            let note =
                if column == Column::Author && !app.shows_author() && !app.hidden.contains(&column)
                {
                    " (every PR has the same one)"
                } else {
                    ""
                };
            let mark = if app.hidden.contains(&column) {
                "[ ]"
            } else {
                "[x]"
            };
            let line = Line::from(format!("{mark} {}{note}", column.name()));
            if i == at { line.reversed() } else { line }
        })
        .collect();
    let buttons = button_row(["OK"], 0);
    let footer = vec![Line::default(), buttons.line];
    let popup_ = Popup {
        title: "Columns".to_string(),
        dry_run: false,
        lines,
        wrap: false,
        footer,
        buttons: buttons.widths,
    };
    popup(frame, area, popup_)
}

fn confirm_dialog(confirm: &Confirm, dry_run: bool, frame: &mut Frame, area: Rect) -> Vec<Rect> {
    let mut lines: Vec<Line> = confirm
        .prs
        .iter()
        .map(|pr| {
            Line::from(vec![
                format!("#{:<6} ", pr.number).into(),
                Span::styled(
                    format!("{:<12} ", pr.checks.label()),
                    state_style(pr.checks),
                ),
                pr.title.as_str().into(),
            ])
        })
        .collect();
    if confirm.prs.is_empty() {
        lines.push(Line::from("Nothing to review."));
    }
    let mut footer = Vec::new();
    if !confirm.left_out.is_empty() {
        footer.push(Line::default());
        footer.push(
            Line::from(format!(
                "Left out closed or merged: {}",
                pr_list(&confirm.left_out)
            ))
            .yellow(),
        );
    }
    footer.push(Line::default());
    let labels = confirm.choices().iter().map(|choice| choice.label());
    let buttons = button_row(labels, confirm.choice);
    footer.push(buttons.line);

    let title = format!("Review {}", count(confirm.prs.len(), "PR"));
    // One PR per line: long titles are cut off rather than wrapped.
    let dialog = Popup {
        title,
        dry_run,
        lines,
        wrap: false,
        footer,
        buttons: buttons.widths,
    };
    popup(frame, area, dialog)
}

fn open_dialog(confirm: &ConfirmOpen, frame: &mut Frame, area: Rect) -> Vec<Rect> {
    let lines = vec![Line::from(pr_list(&confirm.numbers))];
    let highlighted = if confirm.open { 0 } else { 1 };
    let buttons = button_row(["Open", "Cancel"], highlighted);
    let footer = vec![Line::default(), buttons.line];
    let title = format!(
        "Open {} in the browser?",
        count(confirm.numbers.len(), "PR")
    );
    let dialog = Popup {
        title,
        dry_run: false,
        lines,
        wrap: true,
        footer,
        buttons: buttons.widths,
    };
    popup(frame, area, dialog)
}

fn help_popup(offset: usize, frame: &mut Frame, area: Rect) -> (Vec<Rect>, PopupView) {
    let mut lines = Vec::new();
    for (pane, title) in keys::GROUPS {
        let mut group = keys::group(pane).peekable();
        if group.peek().is_none() {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::from(title).bold());
        lines.extend(group.map(|help| Line::from(format!("{:<16} {}", help.keys, help.action))));
    }
    let buttons = button_row(["OK"], 0);
    let footer = vec![Line::default(), buttons.line];
    let help = Popup {
        title: "Keys".to_string(),
        dry_run: false,
        lines,
        wrap: false,
        footer,
        buttons: buttons.widths,
    };
    popup_scrolled(frame, area, help, offset)
}

fn running_popup(
    batch: &Batch,
    dry_run: bool,
    spinner: &str,
    frame: &mut Frame,
    area: Rect,
) -> Vec<Rect> {
    let (finished, total) = batch.progress();
    let progress = format!("{spinner} {finished}/{total} done");
    let step = if batch.is_merging() {
        "Merging"
    } else {
        "Approving"
    };
    let title = format!("{step} {}", count(total, "PR"));
    let mut lines = vec![Line::from(progress), Line::default()];
    // Failures can be long, so the lines wrap.
    lines.extend(batch.items.iter().map(|item| {
        let status = item.current_status();
        let label = Span::styled(status.label(), item_style(status));
        Line::from(vec![Span::raw(format!("#{:<7}", item.pr.number)), label])
    }));
    let footer = vec![Line::default(), Line::from("q: Cancel").dim().centered()];
    let running = Popup {
        title,
        dry_run,
        lines,
        wrap: true,
        footer,
        buttons: Vec::new(),
    };
    popup(frame, area, running)
}

fn item_style(status: &ItemStatus) -> Style {
    match status {
        ItemStatus::Pending | ItemStatus::Cancelled => Style::new().dim(),
        ItemStatus::Working => Style::new().cyan(),
        ItemStatus::Done => Style::new().green(),
        ItemStatus::AutoMerge | ItemStatus::Interrupted => Style::new().yellow(),
        ItemStatus::Failed(_) => Style::new().red(),
    }
}

fn results_popup(summary: &Summary, dry_run: bool, frame: &mut Frame, area: Rect) -> Vec<Rect> {
    let mut lines = vec![Line::from(format!(
        "Approved: {}/{}",
        summary.approved, summary.total
    ))];
    if let Some((merged, tried)) = summary.merged {
        lines.push(Line::from(format!("Merged: {merged}/{tried}")));
    }
    if !summary.failures.is_empty() {
        lines.push(Line::default());
        for (number, step, error) in &summary.failures {
            lines.push(Line::from(format!("#{number}  ✗ {step} failed: {error}")).red());
        }
    }
    if summary.cancelled > 0 {
        lines.push(Line::default());
        lines.push(
            Line::from(format!(
                "Cancelled: {} not finished",
                count(summary.cancelled, "PR")
            ))
            .dim(),
        );
    }
    if !summary.interrupted.is_empty() {
        lines.push(Line::default());
        lines.push(
            Line::from(format!(
                "Interrupted while running - check GitHub: {}",
                pr_list(&summary.interrupted)
            ))
            .yellow(),
        );
    }
    if !summary.auto_merge.is_empty() {
        lines.push(Line::default());
        lines.push(
            Line::from(format!(
                "Auto-merge queued (checks still pending): {}",
                pr_list(&summary.auto_merge)
            ))
            .yellow(),
        );
    }
    let buttons = button_row(["OK"], 0);
    let footer = vec![Line::default(), buttons.line];
    // Errors can be long, so they wrap.
    let results = Popup {
        title: "Results".to_string(),
        dry_run,
        lines,
        wrap: true,
        footer,
        buttons: buttons.widths,
    };
    popup(frame, area, results)
}

/// What a popup shows.
struct Popup<'a> {
    title: String,
    /// Whether to add ` [DRY RUN]` to the title.
    dry_run: bool,
    lines: Vec<Line<'a>>,
    /// Whether long lines wrap rather than being cut off.
    wrap: bool,
    /// Lines pinned to the bottom, such as buttons.
    footer: Vec<Line<'a>>,
    /// The widths of the buttons on the last footer line, which `button_row` drew centred.
    buttons: Vec<u16>,
}

/// Draws a bordered popup in the middle of `area`. The footer lines are pinned to the bottom so
/// they stay visible even when `lines` doesn't fit. Wrapped lines are sized by estimate, so with
/// `wrap` the popup may cut off the end of `lines`. Returns where its buttons were drawn.
fn popup(frame: &mut Frame, area: Rect, popup: Popup) -> Vec<Rect> {
    popup_scrolled(frame, area, popup, 0).0
}

/// Like [`popup`], with its lines scrolled `offset` rows (clamped), and a scrollbar down its right
/// border while they don't all fit. Also returns how many rows there were and how many fit, for
/// the keys that scroll it.
fn popup_scrolled(
    frame: &mut Frame,
    area: Rect,
    popup: Popup,
    offset: usize,
) -> (Vec<Rect>, PopupView) {
    let Popup {
        mut title,
        dry_run,
        lines,
        wrap,
        footer,
        buttons,
    } = popup;
    if dry_run {
        title.push_str(" [DRY RUN]");
    }
    let full = area;
    let width = area.width.saturating_sub(4).min(MAX_POPUP_WIDTH);
    let inner_width = usize::from(width.saturating_sub(2).max(1));
    let rows: usize = if wrap {
        lines
            .iter()
            .map(|line| line.width().div_ceil(inner_width).max(1))
            .sum()
    } else {
        lines.len()
    };
    let footer_rows = footer.len() as u16;
    let height = (rows as u16 + footer_rows + 2).min(area.height);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let block = Block::bordered()
        .border_style(Style::new().cyan())
        .title(Line::from(title).bold());
    let [content_area, footer_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(footer_rows)])
            .areas(block.inner(area));
    let mut content = Paragraph::new(lines);
    if wrap {
        content = content.wrap(Wrap { trim: false });
    }
    let viewport = usize::from(content_area.height);
    let offset = offset.min(rows.saturating_sub(viewport));
    frame.buffer_mut().set_style(full, Modifier::DIM);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(content.scroll((to_u16(offset), 0)), content_area);
    frame.render_widget(Paragraph::new(footer), footer_area);
    // The bar runs beside the lines only, not the footer.
    let bar_area = Rect {
        height: content_area.height + 2,
        ..area
    };
    scrollbar(frame, bar_area, rows, viewport, offset);
    let view = PopupView {
        content: rows,
        viewport,
        offset,
    };
    (button_areas(&buttons, footer_area), view)
}

/// Where the centred buttons of a row of the given widths are, on the last row of `footer`.
fn button_areas(widths: &[u16], footer: Rect) -> Vec<Rect> {
    if widths.is_empty() || footer.is_empty() {
        return Vec::new();
    }
    let total = widths.iter().sum::<u16>() + BUTTON_GAP * (widths.len() as u16 - 1);
    let mut x = footer.x + footer.width.saturating_sub(total) / 2;
    let y = footer.bottom() - 1;
    widths
        .iter()
        .map(|&width| {
            let area = Rect::new(x, y, width, 1);
            x += width + BUTTON_GAP;
            area
        })
        .collect()
}

/// The colour of a row's checks. The checks of a closed or merged PR no longer matter, so they
/// aren't coloured.
fn checks_style(row: &app::Row) -> Style {
    if row.pr.state == PrState::Open {
        state_style(row.checks.state)
    } else {
        Style::new()
    }
}

/// `#1, #2, #3`.
fn pr_list(numbers: &[u64]) -> String {
    let numbers: Vec<String> = numbers.iter().map(|n| format!("#{n}")).collect();
    numbers.join(", ")
}

/// A row of buttons, centred, with the one at `highlighted` reversed.
fn button_row<'a>(labels: impl IntoIterator<Item = &'a str>, highlighted: usize) -> ButtonRow {
    let mut line = Line::default();
    let mut widths = Vec::new();
    for (i, label) in labels.into_iter().enumerate() {
        if i > 0 {
            line.push_span(" ".repeat(usize::from(BUTTON_GAP)));
        }
        let label = format!("[ {label} ]");
        widths.push(label.chars().count() as u16);
        line.push_span(if i == highlighted {
            label.reversed()
        } else {
            label.into()
        });
    }
    ButtonRow {
        line: line.centered(),
        widths,
    }
}

/// A row of buttons and the width of each, for working out where they were drawn.
struct ButtonRow {
    line: Line<'static>,
    widths: Vec<u16>,
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

/// The local filter box, drawn like the search box.
fn filter(app: &App, frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(2);
    let scroll = app.filter.visual_scroll(usize::from(width));
    let block = pane_block("Filter", app.typing == Typing::Filter);
    let input = Paragraph::new(app.filter.value())
        .scroll((0, scroll as u16))
        .block(block);
    frame.render_widget(input, area);
    if app.typing == Typing::Filter {
        let x = app.filter.visual_cursor().saturating_sub(scroll) as u16;
        frame.set_cursor_position(Position::new(area.x + 1 + x, area.y + 1));
    }
}

/// The box that edits the column regex, with the reason a regex was refused in its title.
fn columns(app: &App, frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(2);
    let scroll = app.columns_input.visual_scroll(usize::from(width));
    let mut title = Line::from("Columns");
    if let Some(error) = &app.columns_error {
        title.push_span(format!(" · {error}").red().bold());
    }
    let input = Paragraph::new(app.columns_input.value())
        .scroll((0, scroll as u16))
        .block(pane_block(title, true));
    frame.render_widget(input, area);
    let x = app.columns_input.visual_cursor().saturating_sub(scroll) as u16;
    frame.set_cursor_position(Position::new(area.x + 1 + x, area.y + 1));
}

fn search(app: &App, frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(2);
    let scroll = app.query.visual_scroll(usize::from(width));
    let mut title = Line::from("Search");
    if app.list.searching {
        title.push_span(format!(" · {} searching", spinner(app)).yellow().bold());
    }
    if app.typing != Typing::Search && app.query_is_unsearched() {
        title.push_span(" · not searched".dim());
    }
    let block = pane_block(title, app.typing == Typing::Search);
    let input = Paragraph::new(app.query.value())
        .scroll((0, scroll as u16))
        .block(block);
    frame.render_widget(input, area);
    if app.typing == Typing::Search {
        let x = app.query.visual_cursor().saturating_sub(scroll) as u16;
        frame.set_cursor_position(Position::new(area.x + 1 + x, area.y + 1));
    }
}

/// The most suggestions shown at once.
const MAX_SUGGESTION_ROWS: u16 = 8;

/// The list of suggestions under the search box, starting under the word they replace, drawn over
/// the table. Not drawn where there is no room for at least one row inside its border.
fn suggestions(app: &App, frame: &mut Frame, search_area: Rect, body: Rect) {
    let Some((completion, selected)) = app.completion() else {
        return;
    };
    // A box squeezed by a very short terminal is not under anything.
    if search_area.height < INPUT_BOX_ROWS {
        return;
    }
    let top = search_area.bottom();
    let space = body.bottom().saturating_sub(top);
    let rows = completion
        .suggestions
        .len()
        .min(usize::from(MAX_SUGGESTION_ROWS))
        .min(usize::from(space.saturating_sub(2)));
    if rows == 0 {
        return;
    }
    let widest = completion
        .suggestions
        .iter()
        .map(|s| s.width())
        .max()
        .unwrap_or(0);
    // A column of padding each side of the text, and the border.
    let width = (widest as u16 + 4).min(search_area.width);
    let before: usize = app
        .query
        .value()
        .chars()
        .take(completion.start())
        .map(|c| c.width().unwrap_or(0))
        .sum();
    let scroll = app
        .query
        .visual_scroll(usize::from(search_area.width.saturating_sub(2)));
    let x = (search_area.x + 1 + before.saturating_sub(scroll) as u16)
        .min(search_area.right().saturating_sub(width));
    let area = Rect::new(x, top, width, rows as u16 + 2);
    // Scrolled just far enough to keep the chosen one in view.
    let first = selected.map_or(0, |selected| (selected + 1).saturating_sub(rows));
    let lines: Vec<Line> = completion
        .suggestions
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(i, suggestion)| {
            let line = Line::from(format!(" {suggestion} "));
            if Some(i) == selected {
                line.reversed()
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(Block::bordered()), area);
}

/// A bordered pane, with a cyan border and title while it has focus.
fn pane_block<'a>(title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
    let block = Block::bordered().title(title);
    // Not `block.cyan()`, which would colour the text inside the pane as well.
    if focused {
        block
            .border_style(Style::new().cyan())
            .title_style(Style::new().cyan())
    } else {
        block
    }
}

/// While `pane` has focus, sets the rows a PgUp/PgDn moves: its `height` less the `kept` rows
/// that stay in view, and at least one.
fn set_page(app: &mut App, pane: Pane, height: u16, kept: u16) {
    if app.focus == pane {
        app.drawn.page = height.saturating_sub(kept).max(1);
    }
}

/// The furthest a pane can scroll: until its last row is at the bottom.
/// A scrollbar down the right border of `area`, drawn only while the pane's `content` rows don't
/// all fit in its `viewport`. `offset` is the first row shown.
fn scrollbar(frame: &mut Frame, area: Rect, content: usize, viewport: usize, offset: usize) {
    let margin = Margin::new(0, 1);
    render_scrollbar(
        frame,
        area.inner(margin),
        ScrollbarOrientation::VerticalRight,
        (content, viewport, offset),
    );
}

/// A scrollbar along the bottom border of `area`, drawn only while the pane's `content` columns
/// don't all fit in its `viewport`. `offset` is the first column shown.
fn hscrollbar(frame: &mut Frame, area: Rect, content: usize, viewport: usize, offset: usize) {
    let margin = Margin::new(1, 0);
    render_scrollbar(
        frame,
        area.inner(margin),
        ScrollbarOrientation::HorizontalBottom,
        (content, viewport, offset),
    );
}

/// Draws a scrollbar on the edge of `area`, which is already inside the corners of the border,
/// for the `(content, viewport, offset)` lengths along its axis.
fn render_scrollbar(
    frame: &mut Frame,
    area: Rect,
    orientation: ScrollbarOrientation,
    (content, viewport, offset): (usize, usize, usize),
) {
    if content <= viewport {
        return;
    }
    // The position runs from 0 to the last offset, which is when the thumb reaches the end.
    let mut state = ScrollbarState::new(content - viewport + 1)
        .viewport_content_length(viewport)
        .position(offset);
    let horizontal = orientation == ScrollbarOrientation::HorizontalBottom;
    let mut bar = Scrollbar::new(orientation)
        .begin_symbol(None)
        .end_symbol(None);
    if horizontal {
        bar = bar
            .thumb_symbol(HORIZONTAL_THUMB)
            .track_symbol(Some(HORIZONTAL_TRACK));
    }
    frame.render_stateful_widget(bar, area, &mut state);
}

fn max_scroll(content_rows: usize, area_rows: u16) -> u16 {
    let max = content_rows.saturating_sub(usize::from(area_rows));
    to_u16(max)
}

/// `title` with the `parts` in brackets after it, or alone when there are none.
fn with_counts(title: &str, parts: &[String]) -> String {
    if parts.is_empty() {
        title.to_string()
    } else {
        format!("{title} ({})", parts.join(", "))
    }
}

/// The list's title, with how many PRs it holds and how many of those the filter shows, and how
/// many are selected: `Pull requests (12 of 30, 3 selected)`. What isn't true is left out.
fn table_title(app: &App) -> String {
    let mut parts = Vec::new();
    if !app.rows.is_empty() {
        if app.visible.len() == app.rows.len() {
            parts.push(app.rows.len().to_string());
        } else {
            parts.push(format!("{} of {}", app.visible.len(), app.rows.len()));
        }
    }
    if !app.selected.is_empty() {
        parts.push(format!("{} selected", app.selected.len()));
    }
    with_counts("Pull requests", &parts)
}

/// A pane's title with how many items it holds, if any.
fn counted_title(title: &str, count: usize) -> String {
    let parts: Vec<String> = (count > 0).then(|| count.to_string()).into_iter().collect();
    with_counts(title, &parts)
}

/// The details of the highlighted PR, once loaded.
fn loaded_details(app: &App) -> Option<&crate::gh::PrDetails> {
    match app.details.get(&app.highlighted()?.pr.number)? {
        Fetch::Loaded(loaded) => Some(&loaded.details),
        _ => None,
    }
}

fn checks(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.checks = area;
    app.drawn.checks_layout = ChecksLayout::default();
    let conflict_row = app
        .highlighted()
        .is_some_and(|r| r.checks.state == CheckState::Conflict);
    let count = app
        .highlighted()
        .map_or(0, |r| r.checks.worst_first().len())
        + usize::from(conflict_row);
    let block = pane_block(counted_title("Checks", count), app.focus == Pane::Checks);
    let inner = block.inner(area);
    set_page(app, Pane::Checks, inner.height, 2);
    frame.render_widget(block, area);
    let Some(row) = app.highlighted() else {
        return;
    };
    // A conflict outranks the checks, so it gets a row of its own at the top.
    let conflict = (row.checks.state == CheckState::Conflict).then_some("Merge conflict");
    let first = usize::from(conflict.is_some());
    let results = row.checks.worst_first();
    let selected = app.scroll.check.min(results.len().saturating_sub(1));
    // The names scroll sideways and the status stays. The gap between the columns is one.
    let names_width = inner.width.saturating_sub(CHECK_STATUS_WIDTH + 1);
    let widest = results
        .iter()
        .map(|result| result.key.width())
        .chain(conflict.map(|name| name.width()))
        .max()
        .unwrap_or(0);
    let x = usize::from(app.scroll.checks_x).min(widest.saturating_sub(usize::from(names_width)));
    let name = |name: &str| model::skip_columns(name, x);
    let conflict = conflict.map(|text| styled_check_row(CheckState::Conflict, name(text)));
    let rows: Vec<Row> = conflict
        .into_iter()
        .chain(results.iter().enumerate().map(|(i, result)| {
            let row = styled_check_row(result.state, name(&result.key));
            if i == selected { row.reversed() } else { row }
        }))
        .collect();
    let count = results.len();
    if rows.is_empty() {
        frame.render_widget(Line::from("No checks."), inner);
        return;
    }
    let rows_count = rows.len();
    // One row of the table is its header.
    let scroll = app
        .scroll
        .checks
        .min(max_scroll(rows.len(), inner.height.saturating_sub(1)));
    let table = Table::new(
        rows,
        [Constraint::Length(CHECK_STATUS_WIDTH), Constraint::Fill(1)],
    )
    .header(Row::new(["Status", "Check"]).bold());
    let mut state = TableState::default().with_offset(usize::from(scroll));
    frame.render_stateful_widget(table, inner, &mut state);
    let viewport = usize::from(inner.height.saturating_sub(1));
    scrollbar(frame, area, rows_count, viewport, usize::from(scroll));
    hscrollbar(frame, area, widest, usize::from(names_width), x);
    app.drawn.checks_layout = ChecksLayout {
        count,
        first,
        viewport: usize::from(inner.height.saturating_sub(1)),
    };
    app.scroll.check = selected;
    app.scroll.checks = scroll;
    app.scroll.checks_x = to_u16(x);
}

/// What the approvers pane says about the highlighted PR: nothing without one, and otherwise who
/// has reviewed and who has been asked to, one to a line, with a mark and a colour for how it
/// stands, as in the checks pane, and what else is worth knowing.
fn approvers_text(app: &App) -> Text<'static> {
    match app.highlighted().map(|row| app.details.get(&row.pr.number)) {
        None => Text::default(),
        Some(None | Some(Fetch::Loading(_))) => Text::from("Loading…"),
        Some(Some(Fetch::Failed(error))) => {
            let error = model::first_line(error);
            Text::from(format!("Failed to load details: {error}")).red()
        }
        Some(Some(Fetch::Loaded(Details { details, .. }))) => {
            if details.approvers == Approvers::default() {
                Text::from("No reviews or review requests.")
            } else {
                Text::from(approver_lines(&details.approvers))
            }
        }
    }
}

/// The rows the approvers pane would like when it is `width` columns wide, border included: one
/// for each line as wrapped, between [`MIN_APPROVERS_ROWS`] and [`MAX_APPROVERS_ROWS`].
fn approvers_rows(app: &App, width: u16) -> u16 {
    let inner = usize::from(width.saturating_sub(2));
    let lines = model::layout_plain(&approvers_text(app), inner).lines.len();
    (to_u16(lines) + 2).clamp(MIN_APPROVERS_ROWS, MAX_APPROVERS_ROWS)
}

/// The approvers pane, above the diff.
fn approvers(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.approvers = area;
    let count = loaded_details(app).map_or(0, |d| {
        d.approvers.approvals.len() + d.approvers.requests.len()
    });
    let block = pane_block(
        counted_title("Approvers", count),
        app.focus == Pane::Approvers,
    );
    let inner = block.inner(area);
    set_page(app, Pane::Approvers, inner.height, 1);
    frame.render_widget(block, area);
    let text = approvers_text(app);
    // Wrapped here, as with the description, so the scrollbar knows how many rows there are.
    let laid = model::layout_plain(&text, usize::from(inner.width));
    let rows = laid.lines.len();
    let scroll = app.scroll.approvers.min(max_scroll(rows, inner.height));
    frame.render_widget(
        Paragraph::new(Text::from(laid.lines)).scroll((scroll, 0)),
        inner,
    );
    scrollbar(
        frame,
        area,
        rows,
        usize::from(inner.height),
        usize::from(scroll),
    );
    app.scroll.approvers = scroll;
}

/// The approvers pane's lines: those who asked for changes, then those who approved, those whose
/// review was dismissed, and then those still asked, code owners first.
fn approver_lines(approvers: &Approvers) -> Vec<Line<'static>> {
    let name = |reviewer: &Reviewer| match reviewer {
        Reviewer::Person(login) => format!("@{login}"),
        Reviewer::Team(slug) => slug.clone(),
        Reviewer::Hidden => "a team or reviewer you can't see".to_string(),
    };
    let mut lines = Vec::new();
    for verdict in [
        Verdict::ChangesRequested,
        Verdict::Approved,
        Verdict::Dismissed,
    ] {
        for approval in approvers.approvals.iter().filter(|a| a.verdict == verdict) {
            // A review of an older commit may not count, so it is yellow like a pending check.
            let (mark, label, style) = match (verdict, approval.stale) {
                (Verdict::ChangesRequested, _) => ("✗", "changes requested", Style::new().red()),
                (Verdict::Approved, false) => ("✓", "approved", Style::new().green()),
                (Verdict::Approved, true) => ("✓", "approved", Style::new().yellow()),
                _ => ("○", "dismissed", Style::new().dim()),
            };
            let mut detail = vec![label.to_string()];
            if approval.stale {
                detail.push("on an older commit".to_string());
            }
            if !approval.for_teams.is_empty() {
                detail.push(format!("for {}", approval.for_teams.join(", ")));
            }
            lines.push(Line::from(vec![
                Span::styled(format!("{mark} "), style),
                Span::raw(format!("{} · ", name(&approval.by))),
                Span::styled(detail.join(" · "), style),
            ]));
        }
    }
    let code_owners = approvers.requests.iter().filter(|r| r.code_owner);
    let others = approvers.requests.iter().filter(|r| !r.code_owner);
    for request in code_owners.chain(others) {
        let label = if request.code_owner {
            "waiting · code owner"
        } else {
            "waiting"
        };
        let style = Style::new().yellow();
        lines.push(Line::from(vec![
            Span::styled("… ", style),
            Span::raw(format!("{} · ", name(&request.of))),
            Span::styled(label, style),
        ]));
    }
    lines
}

/// A row of the checks pane, with its status coloured.
fn styled_check_row<'a>(state: CheckState, name: String) -> Row<'a> {
    Row::new([
        Cell::new(state.label()).style(state_style(state)),
        Cell::new(name),
    ])
}

/// The comments of the highlighted PR, as one scrolling document of headers that expand to show
/// the comment under them.
fn comments(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.comments = area;
    app.drawn.comment_layout = CommentLayout::default();
    app.drawn.comment_links = Vec::new();
    let count = loaded_details(app).map_or(0, |d| d.comments.len());
    let block = pane_block(
        counted_title("Comments", count),
        app.focus == Pane::Comments,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (comments, bodies) = match app.highlighted().map(|row| app.details.get(&row.pr.number)) {
        None => return,
        Some(None | Some(Fetch::Loading(_))) => {
            frame.render_widget(Line::from("Loading…"), inner);
            return;
        }
        Some(Some(Fetch::Failed(error))) => {
            let error = model::first_line(error);
            let text = Text::from(format!("Failed to load details: {error}")).red();
            frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            return;
        }
        Some(Some(Fetch::Loaded(Details {
            details,
            comment_bodies,
            ..
        }))) => (&details.comments, comment_bodies),
    };
    if comments.is_empty() {
        frame.render_widget(Line::from("No comments."), inner);
        return;
    }
    let selected = app.scroll.comment.min(comments.len() - 1);
    let width = usize::from(inner.width);
    let widest = (0..comments.len())
        .filter(|i| app.scroll.comments_open.contains(i))
        .map(|i| model::widest_table(&bodies[i].text))
        .max()
        .unwrap_or(0);
    let x = usize::from(app.scroll.comment_x).min(widest.saturating_sub(width));

    let (document, items, links) = comment_document(
        comments,
        bodies,
        &app.scroll.comments_open,
        selected,
        inner.width,
        x,
    );
    let total = items.last().map_or(0, |&(start, height)| start + height);
    // Already wrapped, so the links can be found from where they are drawn.
    let pane = Paragraph::new(Text::from(document));
    let scroll = app.scroll.comments.min(max_scroll(total, inner.height));
    frame.render_widget(pane.scroll((scroll, 0)), inner);
    scrollbar(
        frame,
        area,
        total,
        usize::from(inner.height),
        usize::from(scroll),
    );

    set_page(app, Pane::Comments, inner.height, 1);
    app.drawn.comment_layout = CommentLayout {
        items,
        viewport: usize::from(inner.height),
    };
    app.drawn.comment_links = links;
    app.scroll.comment = selected;
    app.scroll.comment_x = to_u16(x);
    app.scroll.comments = scroll;
}

/// The comments as one document: a header per comment, and the body under it when it is open.
/// Also each comment's first line and height, counted as wrapped to `width`, as that layout is
/// what `update` steps through.
fn comment_document(
    comments: &[Comment],
    bodies: &[model::Markdown],
    open: &HashSet<usize>,
    selected: usize,
    width: u16,
    x: usize,
) -> (
    Vec<Line<'static>>,
    Vec<(usize, usize)>,
    Vec<Vec<model::LinkSpan>>,
) {
    let now = jiff::Timestamp::now();
    let width = usize::from(width);
    let mut document = Vec::new();
    let mut links = Vec::new();
    let mut items = Vec::with_capacity(comments.len());
    for (i, comment) in comments.iter().enumerate() {
        let is_open = open.contains(&i);
        let author = comment
            .author
            .as_ref()
            .map_or("ghost", |a| a.login.as_str());
        let marker = if is_open { '▼' } else { '▶' };
        // Who, what it is if it isn't a plain comment, and when.
        let mut parts = vec![format!("@{author}")];
        parts.extend(comment.tags());
        parts.push(model::ago(&comment.created_at, now));
        let mut header = Line::from(format!("{marker} {}", parts.join(" · ")));
        if i == selected {
            header = header.reversed();
        }
        let start = document.len();
        let (rows, row_links) = model::wrap_line(&header, &[], width);
        document.extend(rows);
        links.extend(row_links);
        if is_open {
            let laid = model::layout_markdown(&bodies[i], width, x);
            document.extend(laid.lines);
            links.extend(laid.links);
            document.push(Line::default());
            links.push(Vec::new());
        }
        items.push((start, document.len() - start));
    }
    (document, items, links)
}

fn diff(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.diff = area;
    let block = pane_block("Diff", app.focus == Pane::Diff);
    let inner = block.inner(area);
    set_page(app, Pane::Diff, inner.height, 1);
    let diff = app
        .highlighted()
        .and_then(|row| app.diffs.get(&row.pr.number));
    let mut scroll = 0;
    let mut x = 0;
    let mut widest = 0;
    let mut lines = 0;
    // A failure is a message, so it wraps; a diff is cut off at the pane's edge.
    let mut wrap = false;
    let text = match diff {
        None if app.highlighted().is_none() => Text::default(),
        // A PR waiting for a free fetch slot has no entry yet.
        None | Some(Fetch::Loading(_)) => Text::from("Loading…"),
        Some(Fetch::Loaded(diff)) => {
            lines = diff.text.lines.len();
            widest = diff.widest;
            scroll = app
                .scroll
                .diff
                .min(max_scroll(diff.text.lines.len(), inner.height));
            x = app.scroll.diff_x.min(max_scroll(diff.widest, inner.width));
            // Only the rows that fit are copied, as a diff can be very long.
            Text::from(
                diff.text
                    .lines
                    .iter()
                    .skip(usize::from(scroll))
                    .take(usize::from(inner.height))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        }
        Some(Fetch::Failed(error)) => {
            wrap = true;
            let error = model::first_line(error);
            Text::from(format!("Failed to load diff: {error}")).red()
        }
    };
    let mut pane = Paragraph::new(text).scroll((0, x)).block(block);
    if wrap {
        pane = pane.wrap(Wrap { trim: false });
    }
    frame.render_widget(pane, area);
    scrollbar(
        frame,
        area,
        lines,
        usize::from(inner.height),
        usize::from(scroll),
    );
    hscrollbar(
        frame,
        area,
        widest,
        usize::from(inner.width),
        usize::from(x),
    );
    app.scroll.diff = scroll;
    app.scroll.diff_x = x;
}

/// The highlighted PR's title and its summary line: checks, review, size and branch.
fn pull_request(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::bordered().title("Pull request");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(row) = app.highlighted() else {
        return;
    };
    let mut summary = vec![
        Span::styled(row.checks_label(), checks_style(row)),
        Span::raw("  "),
        Span::raw(model::review_label(row.pr.review_decision)),
    ];
    // The size and branch come with the details.
    match app.details.get(&row.pr.number) {
        Some(Fetch::Loaded(Details { details, .. })) => summary.extend([
            Span::raw("  "),
            Span::styled(format!("+{}", details.additions), Style::new().green()),
            Span::raw(" "),
            Span::styled(format!("-{}", details.deletions), Style::new().red()),
            Span::raw(" lines  "),
            Span::raw(details.head_ref_name.as_str()),
        ]),
        None | Some(Fetch::Loading(_)) => summary.push(Span::raw("  Loading…")),
        Some(Fetch::Failed(_)) => {}
    }
    let title = Line::from(format!("#{} {}", row.pr.number, row.pr.title)).bold();
    frame.render_widget(Paragraph::new(vec![title, Line::from(summary)]), inner);
}

fn description(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.description = area;
    app.drawn.description_links = Vec::new();
    // The block's inner area doesn't depend on its title.
    let inner = pane_block("", false).inner(area);
    set_page(app, Pane::Description, inner.height, 1);
    let width = usize::from(inner.width);
    let mut widest = 0;
    let mut x = 0;
    let laid = match app.highlighted() {
        None => model::Laid::default(),
        Some(row) => match app.details.get(&row.pr.number) {
            None | Some(Fetch::Loading(_)) => model::layout_plain(&Text::from("Loading…"), width),
            Some(Fetch::Loaded(Details { body, .. })) => {
                widest = model::widest_table(&body.text);
                x = usize::from(app.scroll.description_x).min(widest.saturating_sub(width));
                model::layout_markdown(body, width, x)
            }
            Some(Fetch::Failed(error)) => {
                let error = model::first_line(error);
                let text = Text::from(format!("Failed to load details: {error}")).red();
                model::layout_plain(&text, width)
            }
        },
    };
    app.scroll.description_x = to_u16(x);
    let title = if widest > width {
        "Description (←/→ scroll tables)"
    } else {
        "Description"
    };
    let block = pane_block(title, app.focus == Pane::Description);
    let lines = laid.lines.len();
    let scroll = app.scroll.description.min(max_scroll(lines, inner.height));
    // Already wrapped, so the links can be found from where they are drawn.
    let pane = Paragraph::new(Text::from(laid.lines)).scroll((scroll, 0));
    frame.render_widget(pane.block(block), area);
    scrollbar(
        frame,
        area,
        lines,
        usize::from(inner.height),
        usize::from(scroll),
    );
    app.scroll.description = scroll;
    app.drawn.description_links = laid.links;
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;

    use super::*;
    use crate::app::Msg;
    use crate::gh::Pr;
    use crate::testing::{app_with, ctrl_g, pr_with, press};

    fn pr(number: u64, title: &str, state: &str, review: &str) -> Pr {
        pr_with(
            number,
            title,
            state,
            review,
            "MERGEABLE",
            serde_json::json!([]),
        )
    }

    /// Draws the app on a screen of this size, as the real terminal would show it.
    fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| view(app, frame)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The text of each screen row.
    fn lines(buffer: &Buffer) -> Vec<String> {
        let width = usize::from(buffer.area.width);
        buffer
            .content()
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    /// The position of the first cell of the first occurrence of `text`. Read its colours with
    /// `buffer[position]`.
    fn find(buffer: &Buffer, text: &str) -> Option<Position> {
        lines(buffer).iter().enumerate().find_map(|(y, line)| {
            let column = line.find(text).map(|byte| line[..byte].chars().count())?;
            Some(Position::new(column as u16, y as u16))
        })
    }

    #[test]
    fn the_sorted_columns_header_has_an_arrow_for_the_direction_and_s_changes_the_rows_drawn() {
        let mut app = app_with(vec![
            pr(9, "Bump zulu", "OPEN", ""),
            pr(3, "Bump alpha", "OPEN", ""),
        ]);
        // The header row of the table: the one with the `#` column.
        let header = |app: &mut App| {
            let screen = lines(&draw(app, 200, 30));
            screen.into_iter().find(|l| l.contains(" # ")).unwrap()
        };
        let row = |screen: &[String], number: &str| {
            screen
                .iter()
                .position(|line| line.contains(number) && line.contains("[ ]"))
                .unwrap()
        };
        // Sorted by checks to begin with, smallest first.
        let top = header(&mut app);
        assert!(top.contains("Checks ▲"), "{top}");
        assert!(!top.contains("Review ▲") && !top.contains('▼'), "{top}");
        // The same checks, so GitHub's order: 9, then 3.
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(row(&screen, "#9") < row(&screen, "#3"));

        press(&mut app, "s");
        let top = header(&mut app);
        assert!(top.contains("# ▲") && !top.contains("Checks ▲"), "{top}");
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(row(&screen, "#3") < row(&screen, "#9"));

        press(&mut app, "S");
        let top = header(&mut app);
        assert!(top.contains("# ▼"), "{top}");
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(row(&screen, "#9") < row(&screen, "#3"));

        press(&mut app, "s");
        assert!(header(&mut app).contains("Title ▲"));
        press(&mut app, "s");
        assert!(header(&mut app).contains("Review ▲"));
        press(&mut app, "s");
        assert!(header(&mut app).contains("Author ▲"));
        // The update time has a column to mark now.
        press(&mut app, "s");
        assert!(header(&mut app).contains("Updated ▲"));
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(!screen[0].contains("sorted by"), "{}", screen[0]);
    }

    /// The middle of the header cell that sorts by `sort`, as last drawn.
    fn header_cell(app: &App, sort: Sort) -> Position {
        let (area, _) = app
            .drawn
            .areas
            .table_header
            .iter()
            .find(|(_, s)| *s == sort)
            .unwrap_or_else(|| panic!("no header for {sort:?}"));
        Position::new(area.x + area.width / 2, area.y)
    }

    fn click_header(app: &mut App, sort: Sort) {
        let at = header_cell(app, sort);
        click(app, at);
    }

    #[test]
    fn the_sortable_headers_are_where_they_are_drawn() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        let buffer = draw(&mut app, 200, 30);
        let header = &app.drawn.areas.table_header;
        let sorts: Vec<Sort> = header.iter().map(|(_, sort)| *sort).collect();
        assert_eq!(
            sorts,
            [
                Sort::Number,
                Sort::Checks,
                Sort::Review,
                Sort::Author,
                Sort::Updated,
                Sort::Title
            ]
        );
        // Each is on the header row, over its own text, and none overlap.
        let text_at = |area: &Rect| -> String {
            (area.x..area.right())
                .map(|x| buffer[(x, area.y)].symbol())
                .collect()
        };
        assert!(text_at(&header[0].0).trim().starts_with('#'));
        assert!(text_at(&header[1].0).trim().starts_with("Checks"));
        assert!(text_at(&header[2].0).trim().starts_with("Review"));
        assert!(text_at(&header[3].0).trim().starts_with("Author"));
        assert!(text_at(&header[4].0).trim().starts_with("Updated"));
        assert!(text_at(&header[5].0).trim().starts_with("Title"));
        for pair in header.windows(2) {
            assert!(pair[0].0.right() <= pair[1].0.x);
            assert_eq!(pair[0].0.y, pair[1].0.y);
        }
        // The title columns reach the end of the table.
        let title = header[5].0;
        assert_eq!(title.right(), app.drawn.areas.table.right() - 1);
    }

    #[test]
    fn clicking_a_header_sorts_by_it_then_reverses_it_then_goes_back_to_the_default() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 200, 30);
        // The default is checks, smallest first, so the first click on it reverses it.
        for expected in [
            (Sort::Checks, true),
            (Sort::Checks, false),
            (Sort::Checks, true),
        ] {
            click_header(&mut app, Sort::Checks);
            draw(&mut app, 200, 30);
            assert_eq!(app.sort(), expected);
        }
        // Another column: smallest first, then reversed, then back to the default.
        for expected in [
            (Sort::Review, false),
            (Sort::Review, true),
            (Sort::Checks, false),
        ] {
            click_header(&mut app, Sort::Review);
            draw(&mut app, 200, 30);
            assert_eq!(app.sort(), expected);
        }
        for expected in [(Sort::Number, false), (Sort::Number, true)] {
            click_header(&mut app, Sort::Number);
            draw(&mut app, 200, 30);
            assert_eq!(app.sort(), expected);
        }
        // Any title column is the title; clicking another column changes over.
        click_header(&mut app, Sort::Title);
        draw(&mut app, 200, 30);
        assert_eq!(app.sort(), (Sort::Title, false));
    }

    #[test]
    fn a_click_on_a_header_reorders_the_rows_and_moves_the_arrow() {
        let mut app = app_with(vec![
            by("alice", 9, "Bump zulu"),
            by("alice", 3, "Bump alpha"),
        ]);
        draw(&mut app, 200, 30);
        click_header(&mut app, Sort::Number);
        let screen = lines(&draw(&mut app, 200, 30));
        let header = screen.iter().find(|l| l.contains(" # ")).unwrap();
        assert!(
            header.contains("# ▲") && !header.contains("Checks ▲"),
            "{header}"
        );
        let row = |number: &str| {
            screen
                .iter()
                .position(|line| line.contains(number) && line.contains("[ ]"))
                .unwrap()
        };
        assert!(row("#3") < row("#9"));
        // The highlight stayed on the PR it was on, which was the first listed.
        assert_eq!(app.highlighted().unwrap().pr.number, 9);
        assert!(app.focus == Pane::Table);
    }

    #[test]
    fn a_click_on_the_checkbox_header_does_not_sort_and_one_on_the_author_does() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 200, 30);
        let table = app.drawn.areas.table;
        // The checkbox is the first column inside the border.
        click(&mut app, Position::new(table.x + 1, table.y + 1));
        assert_eq!(app.sort(), (Sort::Checks, false));
        for expected in [
            (Sort::Author, false),
            (Sort::Author, true),
            (Sort::Checks, false),
        ] {
            click_header(&mut app, Sort::Author);
            draw(&mut app, 200, 30);
            assert_eq!(app.sort(), expected);
        }
    }

    #[test]
    fn there_is_no_author_header_to_click_when_the_search_names_one_author() {
        let mut app = app_for_query("is:pr author:alice", vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 200, 30);
        let sorts: Vec<Sort> = app
            .drawn
            .areas
            .table_header
            .iter()
            .map(|(_, s)| *s)
            .collect();
        assert_eq!(
            sorts,
            [
                Sort::Number,
                Sort::Checks,
                Sort::Review,
                Sort::Updated,
                Sort::Title
            ]
        );
    }

    /// The header row of the table, as drawn at 200 columns.
    fn table_header_text(app: &mut App) -> String {
        let buffer = draw(app, 200, 30);
        let area = app.drawn.areas.table;
        (area.x..area.right())
            .map(|x| buffer[(x, area.y + 1)].symbol())
            .collect()
    }

    #[test]
    fn the_updated_column_shows_how_long_ago_in_a_word_or_two() {
        let mut pr = by("alice", 1, "Bump alpha");
        pr.updated_at = "2020-01-02T03:04:05Z".to_string();
        let mut app = app_with(vec![pr]);
        let screen = lines(&draw(&mut app, 200, 30)).join("\n");
        assert!(screen.contains("Updated"), "{screen}");
        assert!(screen.contains('y'), "{screen}");
    }

    #[test]
    fn each_column_can_be_hidden_and_takes_its_header_and_click_area_with_it() {
        for column in Column::ALL {
            let mut app = app_with(vec![by("alice", 1, "Bump alpha"), by("bob", 2, "Bump b")]);
            app.toggle_column(column);
            let top = table_header_text(&mut app);
            assert!(!top.contains(column.name()), "{column:?}: {top}");
            // The others are still there, and a click on each still sorts by it.
            let sorts: Vec<Sort> = app
                .drawn
                .areas
                .table_header
                .iter()
                .map(|(_, s)| *s)
                .collect();
            assert!(!sorts.contains(&column.sort()), "{column:?}");
            assert_eq!(sorts.len(), Column::ALL.len(), "{column:?}: {sorts:?}");
            for sort in sorts {
                click_header(&mut app, sort);
                draw(&mut app, 200, 30);
                assert_eq!(app.sort().0, sort, "{column:?}");
            }
        }
    }

    #[test]
    fn hiding_the_number_unfreezes_it_and_the_checkbox_alone_stays() {
        let mut app = app_with_a_long_title();
        app.toggle_column(Column::Number);
        draw(&mut app, 100, 40);
        for _ in 0..8 {
            press(&mut app, "l");
        }
        draw(&mut app, 100, 40);
        let table = app.drawn.areas.table;
        assert!(app.list_x > 0);
        for (area, sort) in &app.drawn.areas.table_header {
            assert!(area.x > table.x + CHECKBOX_WIDTH, "{sort:?}");
        }
    }

    #[test]
    fn the_columns_popup_shows_every_column_and_space_hides_the_highlighted_one() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha"), by("bob", 2, "Bump b")]);
        press(&mut app, "v");
        let screen = lines(&draw(&mut app, 200, 30)).join("\n");
        assert!(
            screen.contains("[x] Number") || screen.contains("[x] #"),
            "{screen}"
        );
        for column in Column::ALL {
            assert!(
                screen.contains(&format!("[x] {}", column.name())),
                "{screen}"
            );
        }
        press(&mut app, "j");
        press(&mut app, " ");
        assert_eq!(app.hidden, [Column::Checks]);
        let screen = lines(&draw(&mut app, 200, 30)).join("\n");
        assert!(screen.contains("[ ] Checks"), "{screen}");
        press(&mut app, " ");
        assert!(app.hidden.is_empty());
        // It takes every key but the ones that close it, and Esc closes it.
        press(&mut app, "s");
        assert!(matches!(app.mode, Mode::PickColumns(_)));
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn the_filter_matches_only_the_columns_shown() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha"), by("bob", 2, "Bump b")]);
        app.filter = tui_input::Input::new("bob".to_string());
        app.toggle_column(Column::Number);
        assert_eq!(app.visible.len(), 1);
        app.toggle_column(Column::Author);
        assert!(app.visible.is_empty());
        app.toggle_column(Column::Author);
        assert_eq!(app.visible.len(), 1);
    }

    /// The text of the screen, which has each pane's title in its top border.
    fn border_titles(app: &mut App, width: u16) -> String {
        lines(&draw(app, width, 40)).join("\n")
    }

    #[test]
    fn the_list_title_counts_the_prs_the_filter_shows_and_those_selected() {
        for width in [200, 100] {
            let mut app = app_with(vec![
                by("alice", 1, "Bump alpha"),
                by("alice", 2, "Bump beta"),
                by("bob", 3, "Fix gamma"),
            ]);
            let screen = border_titles(&mut app, width);
            assert!(screen.contains("Pull requests (3)"), "{width}: {screen}");
            assert!(!screen.contains(" of 3"), "{width}");
            press(&mut app, " ");
            let screen = border_titles(&mut app, width);
            assert!(
                screen.contains("Pull requests (3, 1 selected)"),
                "{width}: {screen}"
            );
            press(&mut app, "fbump");
            key(&mut app, KeyCode::Enter);
            let screen = border_titles(&mut app, width);
            assert!(
                screen.contains("Pull requests (2 of 3, 1 selected)"),
                "{width}: {screen}"
            );
        }
    }

    #[test]
    fn the_checks_comments_and_approvers_titles_say_how_many_they_hold() {
        for width in [200, 100] {
            let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
            // Nothing loaded yet: no count to claim.
            let screen = border_titles(&mut app, width);
            assert!(
                screen.contains("Comments") && !screen.contains("Comments ("),
                "{screen}"
            );
            assert!(
                screen.contains("Approvers") && !screen.contains("Approvers ("),
                "{screen}"
            );
            let details = serde_json::from_value(serde_json::json!({
                "headRefName": "branch",
                "body": "",
                "comments": [
                    {"author": {"login": "a"}, "createdAt": "2026-01-01T00:00:00Z", "body": "x"},
                    {"author": {"login": "b"}, "createdAt": "2026-01-02T00:00:00Z", "body": "y"},
                ],
                "additions": 1,
                "deletions": 1,
                "approvers": {"requests": [
                    {"of": {"Person": "u1"}, "code_owner": false},
                    {"of": {"Person": "u2"}, "code_owner": false},
                    {"of": {"Person": "u3"}, "code_owner": false},
                ]},
            }))
            .unwrap();
            app.details_loaded(1, Ok(details));
            let screen = border_titles(&mut app, width);
            assert!(screen.contains("Comments (2)"), "{width}: {screen}");
            assert!(screen.contains("Approvers (3)"), "{width}: {screen}");
        }
    }

    #[test]
    fn the_headers_follow_a_table_that_scrolls_sideways_and_stay_clear_of_the_frozen_columns() {
        let mut app = app_with_a_long_title();
        draw(&mut app, 100, 40);
        let before = app.drawn.areas.table_header.clone();
        let number = before.iter().find(|(_, s)| *s == Sort::Number).unwrap().0;
        for _ in 0..8 {
            press(&mut app, "l");
        }
        draw(&mut app, 100, 40);
        assert!(app.list_x > 0);
        let table = app.drawn.areas.table;
        let after = &app.drawn.areas.table_header;
        // The number is frozen and stays; the others have moved left, or are out of sight, and none
        // reach under the frozen columns or past the table.
        let number_after = after.iter().find(|(_, s)| *s == Sort::Number).unwrap().0;
        assert_eq!(number_after, number);
        for (area, sort) in after {
            if *sort != Sort::Number {
                assert!(area.x >= table.x + 1 + FROZEN_WIDTH, "{sort:?}");
            }
            assert!(area.right() <= table.right() - 1, "{sort:?}");
        }
        // The title is still there to click, and sorts by title.
        click_header(&mut app, Sort::Title);
        assert_eq!(app.sort(), (Sort::Title, false));
    }

    #[test]
    fn there_are_no_headers_to_click_with_no_rows_and_a_dialog_ignores_clicks() {
        let mut app = app_with(vec![]);
        draw(&mut app, 200, 30);
        assert!(app.drawn.areas.table_header.is_empty());

        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 200, 30);
        let cell = header_cell(&app, Sort::Number);
        press(&mut app, "?");
        click(&mut app, cell);
        assert_eq!(app.sort(), (Sort::Checks, false));
    }

    #[test]
    fn a_title_sort_marks_the_first_title_column_when_there_are_several() {
        let mut app = app_with(vec![pr(1, "Bump alpha to 1.2", "OPEN", "")]);
        app.columns =
            Some(model::parse_column_regex(r"Bump (?<container>.+?) to (?<version>\S+)$").unwrap());
        press(&mut app, "ss");
        let screen = lines(&draw(&mut app, 200, 30));
        let top = screen.iter().find(|l| l.contains(" # ")).unwrap();
        assert!(
            top.contains("container ▲") && !top.contains("version ▲"),
            "{top}"
        );
    }

    #[test]
    fn draws_the_table() {
        let mut app = app_with(vec![
            pr(7, "Bump example-lib", "OPEN", "APPROVED"),
            pr(9, "Old change", "MERGED", ""),
        ]);
        let buffer = draw(&mut app, 200, 30);
        let screen = lines(&buffer);

        assert!(screen[0].starts_with("PRs"));
        let header = screen.iter().find(|line| line.contains("Checks")).unwrap();
        assert!(header.contains("# ") && header.contains("Review") && header.contains("Title"));
        let open = screen
            .iter()
            .find(|line| line.contains("#7") && line.contains("["))
            .unwrap();
        assert!(open.contains("[ ]"));
        assert!(open.contains("· no checks"));
        assert!(open.contains("✓ approved"));
        assert!(open.contains("Bump example-lib"));
        let merged = screen
            .iter()
            .find(|line| line.contains("#9") && line.contains("["))
            .unwrap();
        assert!(merged.contains("· merged") && merged.contains("Old change"));

        // The helper finds cells, so a test can read their colours.
        let position = find(&buffer, "Bump example-lib").unwrap();
        assert!(screen[usize::from(position.y)].contains("#7"));
        assert_eq!(buffer[position].symbol(), "B");
    }

    fn run(name: &str, status: &str, conclusion: &str) -> serde_json::Value {
        serde_json::json!({
            "__typename": "CheckRun",
            "name": name,
            "workflowName": "CI",
            "status": status,
            "conclusion": conclusion,
            "startedAt": "2026-01-02T03:00:00Z",
        })
    }

    fn pr_checked(number: u64, mergeable: &str, checks: Vec<serde_json::Value>) -> Pr {
        pr_with(
            number,
            &format!("PR {number}"),
            "OPEN",
            "",
            mergeable,
            checks.into(),
        )
    }

    /// Five open PRs, one in each state. The table puts them in this order: passing first, then
    /// pending and none, then failing and conflict.
    fn one_of_each() -> App {
        app_with(vec![
            pr_checked(1, "MERGEABLE", vec![run("build", "COMPLETED", "SUCCESS")]),
            pr_checked(2, "MERGEABLE", vec![run("build", "IN_PROGRESS", "")]),
            pr_checked(3, "MERGEABLE", vec![]),
            pr_checked(4, "MERGEABLE", vec![run("build", "COMPLETED", "FAILURE")]),
            pr_checked(5, "CONFLICTING", vec![run("build", "COMPLETED", "SUCCESS")]),
        ])
    }

    #[test]
    fn colours_the_checks_column() {
        let mut app = one_of_each();
        let buffer = draw(&mut app, 200, 30);
        for (label, colour) in [
            ("✓ passing", Color::Green),
            ("… pending", Color::Yellow),
            ("· no checks", Color::Yellow),
            ("✗ failing", Color::Red),
            ("✗ conflict", Color::Red),
        ] {
            // The checks pane lists the highlighted PR's state too, so look in the table's rows.
            let table_row = lines(&buffer)
                .iter()
                .position(|line| line.contains(label) && line.contains('#'))
                .unwrap_or_else(|| panic!("no table row for {label}"));
            let column = lines(&buffer)[table_row].find(label).unwrap();
            let position = Position::new(column as u16, table_row as u16);
            assert_eq!(buffer[position].fg, colour, "{label}");
        }
    }

    #[test]
    fn colours_the_checks_pane_and_shows_a_conflict_first() {
        let mut app = one_of_each();
        // The conflicting PR is last.
        press(&mut app, "jjjj");
        let buffer = draw(&mut app, 200, 40);
        let screen = lines(&buffer);

        let conflict = find(&buffer, "Merge conflict").unwrap();
        let status = Position::new(conflict.x - 11, conflict.y);
        assert!(screen[usize::from(status.y)].contains("✗ conflict"));
        assert_eq!(buffer[status].fg, Color::Red);
        // Above the check that passed, which is the only other row.
        let build = find(&buffer, "CI/build").unwrap();
        assert!(build.y > conflict.y);
        let passed = Position::new(build.x - 11, build.y);
        assert!(screen[usize::from(passed.y)].contains("✓ passing"));
        assert_eq!(buffer[passed].fg, Color::Green);
    }

    #[test]
    fn a_conflict_with_no_checks_still_shows_the_conflict() {
        let mut app = app_with(vec![pr_checked(1, "CONFLICTING", vec![])]);
        let buffer = draw(&mut app, 200, 40);
        assert!(find(&buffer, "Merge conflict").is_some());
        assert!(find(&buffer, "No checks.").is_none());
    }

    #[test]
    fn colours_the_checks_in_the_confirm_dialog() {
        let mut app = one_of_each();
        // Select everything, then open the dialog.
        press(&mut app, " j j j j ");
        ctrl_g(&mut app);
        let buffer = draw(&mut app, 200, 40);
        for (label, colour) in [
            ("✓ passing", Color::Green),
            ("… pending", Color::Yellow),
            ("✗ failing", Color::Red),
            ("✗ conflict", Color::Red),
        ] {
            // The table's checks column is in the left 30 columns; the centred dialog is to the
            // right of that.
            let position = (0..buffer.area.height)
                .find_map(|y| {
                    let line: String = lines(&buffer)[usize::from(y)].chars().skip(30).collect();
                    let x = line.find(label).map(|byte| line[..byte].chars().count())?;
                    Some(Position::new(30 + x as u16, y))
                })
                .unwrap_or_else(|| panic!("no dialog row for {label}"));
            assert_eq!(buffer[position].fg, colour, "{label}");
        }
    }

    /// A PR by `login`, and an app that has loaded these PRs for `query`.
    fn by(login: &str, number: u64, title: &str) -> Pr {
        let mut pr = pr(number, title, "OPEN", "");
        pr.author.login = login.to_string();
        pr
    }

    fn app_for_query(query: &str, prs: Vec<Pr>) -> App {
        let mut app = App::new("PRs".to_string(), query.to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs)));
        app
    }

    #[test]
    fn draws_the_author_of_each_pr() {
        let mut app = app_with(vec![
            by("alice", 1, "Bump alpha"),
            by("bob", 2, "Bump beta"),
        ]);
        let screen = lines(&draw(&mut app, 200, 30));

        assert!(screen.iter().any(|l| l.contains("Author")));
        assert!(
            screen
                .iter()
                .any(|l| l.contains("#1") && l.contains("alice"))
        );
        assert!(screen.iter().any(|l| l.contains("#2") && l.contains("bob")));
    }

    #[test]
    fn draws_a_bot_the_way_the_query_spells_it() {
        let mut app = app_with(vec![by("app/renovate", 1, "Bump alpha")]);
        let screen = lines(&draw(&mut app, 200, 30));

        assert!(
            screen
                .iter()
                .any(|l| l.contains("#1") && l.contains("app/renovate"))
        );
    }

    #[test]
    fn hides_the_author_column_when_the_query_names_one_author() {
        let mut app = app_for_query(
            "is:pr author:app/renovate",
            vec![by("app/renovate", 1, "Bump alpha")],
        );
        let screen = lines(&draw(&mut app, 200, 30));

        assert!(!screen.iter().any(|l| l.contains("Author")));
        assert!(
            !screen
                .iter()
                .any(|l| l.contains("[ ]") && l.contains("app/renovate"))
        );
        assert!(
            screen
                .iter()
                .any(|l| l.contains("#1") && l.contains("Bump alpha"))
        );
    }

    #[test]
    fn a_search_term_does_not_hide_the_author_column() {
        let mut app = app_for_query("is:pr alice", vec![by("alice", 1, "Bump alpha")]);
        let screen = lines(&draw(&mut app, 200, 30));

        assert!(screen.iter().any(|l| l.contains("Author")));
    }

    #[test]
    fn a_fuzzy_filter_shows_the_rows_it_matches() {
        let mut app = app_with(vec![
            by("alice", 1, "Bump alpha"),
            by("app/renovate", 2, "Bump beta"),
            by("bob", 3, "Update gamma"),
        ]);
        press(&mut app, "frnv");
        let screen = lines(&draw(&mut app, 200, 30));

        assert!(screen.iter().any(|l| l.contains("Bump beta")));
        assert!(!screen.iter().any(|l| l.contains("Bump alpha")));
        assert!(!screen.iter().any(|l| l.contains("Update gamma")));
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
        assert!(!screen.iter().any(|line| line.contains("Filter")));

        press(&mut app, " jjfgamma");
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(screen.iter().any(|line| line.contains("Filter")));
        assert!(screen.iter().any(|line| line.contains("Update gamma")));
        assert!(!screen.iter().any(|line| line.contains("Bump alpha")));
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

        let header = screen.iter().find(|line| line.contains("Checks")).unwrap();
        assert!(header.contains("container") && header.contains("version"));
        assert!(!header.contains("Title"));
        let bump = screen
            .iter()
            .find(|line| line.contains("#1") && line.contains("["))
            .unwrap();
        assert!(bump.contains("alpha") && bump.contains("1.2"));
        assert!(!bump.contains("Bump"));
        // A title that doesn't match gets empty cells; the title is still not shown.
        let fix = screen
            .iter()
            .find(|line| line.contains("#2") && line.contains("["))
            .unwrap();
        assert!(!fix.contains("Fix beta"));
    }

    /// Clears the columns box and types `regex` into it.
    fn type_columns(app: &mut App, regex: &str) {
        for _ in 0..app.columns_input.value().chars().count() {
            key(app, KeyCode::Backspace);
        }
        press(app, regex);
    }

    #[test]
    fn a_valid_regex_in_the_columns_box_changes_the_headers_on_enter() {
        let mut app = app_with(vec![pr(1, "Bump alpha to 1.2", "OPEN", "")]);
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(screen.iter().any(|line| line.contains("Title")));

        press(&mut app, "c");
        assert!(app.typing == Typing::Columns);
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(screen.iter().any(|line| line.contains("Columns")));
        assert!(app.drawn.areas.columns != Rect::default());

        press(&mut app, r"Bump (?<container>.+?) to (?<version>\S+)$");
        key(&mut app, KeyCode::Enter);
        assert!(app.typing == Typing::Nothing);
        let screen = lines(&draw(&mut app, 200, 30));
        let header = screen.iter().find(|line| line.contains("Checks")).unwrap();
        assert!(header.contains("container") && header.contains("version"));
        assert!(!header.contains("Title"));
        // The box is gone, and has no area to catch a click.
        assert_eq!(app.drawn.areas.columns, Rect::default());

        // Opening it again starts from the regex in use, and an empty one goes back to the title.
        press(&mut app, "c");
        assert_eq!(
            app.columns_input.value(),
            r"Bump (?<container>.+?) to (?<version>\S+)$"
        );
        type_columns(&mut app, "");
        key(&mut app, KeyCode::Enter);
        assert!(app.columns.is_none());
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(screen.iter().any(|line| line.contains("Title")));
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
            let header = screen.iter().find(|line| line.contains("Checks")).unwrap();
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
        draw(&mut app, 200, 40);
        let area = app.drawn.areas.columns;
        click(&mut app, Position::new(area.x + 1 + 2, area.y + 1));
        assert!(app.typing == Typing::Columns);
        assert_eq!(app.columns_input.cursor(), 2);

        // A click on the table leaves the box; the regex was never applied.
        click(&mut app, Position::new(2, area.y + 6));
        assert!(app.typing == Typing::Nothing);
        assert!(app.columns.is_none());
    }

    #[test]
    fn the_filter_matches_the_columns_shown() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha to 1.2", "OPEN", ""),
            pr(2, "Bump beta to 3.4", "OPEN", ""),
        ]);
        app.columns =
            Some(model::parse_column_regex(r"Bump (?<container>.+?) to (?<version>\S+)$").unwrap());

        press(&mut app, "f3.4");
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(screen.iter().any(|line| line.contains("#2")));
        assert!(!screen.iter().any(|line| line.contains("#1")));

        // "Bump" is in the titles but not in any column.
        for _ in 0..3 {
            app.update(Msg::Key(KeyCode::Backspace.into()));
        }
        press(&mut app, "Bump");
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(screen.iter().any(|line| line.contains("No PRs match")));
    }

    #[test]
    fn shows_each_pr_of_a_running_batch_and_what_cancelling_did() {
        let mut app = app_with((1..=6).map(|n| pr(n, "Bump alpha", "OPEN", "")).collect());
        press(&mut app, " j j j j j ");
        ctrl_g(&mut app);
        app.update(Msg::Key(KeyCode::Enter.into()));
        // Four approvals run at once; two wait.
        assert_eq!(app.approvals_to_run().len(), 4);
        let epoch = app.batch_epoch();
        app.update(Msg::Approved(epoch, 1, Ok(())));
        let screen = lines(&draw(&mut app, 200, 30));
        // The popup draws over the table, so a status shares its line with a table row.
        let with = |text: &str| screen.iter().filter(|line| line.contains(text)).count();
        assert_eq!(with("✓ done"), 1);
        assert_eq!(with("… working"), 3);
        assert_eq!(with("· pending"), 2);
        assert!(screen.iter().any(|line| line.contains("q: Cancel")));

        press(&mut app, "q");
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(screen.iter().any(|line| line.contains("Cancelled: 2 PRs")));
        assert!(
            screen
                .iter()
                .any(|line| line.contains("check GitHub: #2, #3, #4"))
        );
    }

    #[test]
    fn the_pull_request_box_shows_the_title_and_a_summary() {
        let mut app = app_with(vec![pr(7, "Bump example-lib", "OPEN", "APPROVED")]);
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(screen.iter().any(|line| line.contains("Pull request")));
        // Until the details load, only what the list knows is shown.
        let summary = screen
            .iter()
            .find(|line| line.contains("✓ approved") && line.contains("no checks"));
        assert!(summary.unwrap().contains("Loading…"));

        let details = serde_json::from_value(serde_json::json!({
            "headRefName": "renovate/example-lib",
            "body": "",
            "comments": [],
            "additions": 12,
            "deletions": 3,
        }))
        .unwrap();
        app.details_loaded(7, Ok(details));
        let buffer = draw(&mut app, 200, 30);
        let screen = lines(&buffer);
        let title = screen
            .iter()
            .find(|line| line.contains("#7 Bump example-lib"))
            .unwrap();
        assert!(!title.contains("[ ]"));
        let summary = screen
            .iter()
            .find(|line| line.contains("+12 -3 lines  renovate/example-lib"))
            .unwrap();
        assert!(summary.contains("· no checks") && summary.contains("✓ approved"));
        let added = find(&buffer, "+12").unwrap();
        assert_eq!(buffer[added].fg, Color::Green);
        let removed = find(&buffer, "-3 lines").unwrap();
        assert_eq!(buffer[removed].fg, Color::Red);
    }

    #[test]
    fn dims_everything_behind_a_popup() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let plain = draw(&mut app, 200, 30);
        let table = find(&plain, "is:pr").unwrap();
        assert!(!plain[table].modifier.contains(Modifier::DIM));

        press(&mut app, "?");
        let buffer = draw(&mut app, 200, 30);
        assert!(buffer[table].modifier.contains(Modifier::DIM));
        let popup = find(&buffer, "Keys").unwrap();
        let inside = Position::new(popup.x + 2, popup.y + 1);
        assert!(!buffer[popup].modifier.contains(Modifier::DIM));
        assert!(!buffer[inside].modifier.contains(Modifier::DIM));
    }

    fn click(app: &mut App, position: Position) {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        app.update(Msg::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: position.x,
            row: position.y,
            modifiers: KeyModifiers::NONE,
        }));
    }

    #[test]
    fn a_click_focuses_the_pane_under_it() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw(&mut app, 200, 40);
        let panes = [
            (Pane::Checks, app.drawn.areas.checks),
            (Pane::Comments, app.drawn.areas.comments),
            (Pane::Description, app.drawn.areas.description),
            (Pane::Diff, app.drawn.areas.diff),
            (Pane::Table, app.drawn.areas.table),
        ];
        for (pane, area) in panes {
            // The border counts as part of the pane, so click on it and inside it.
            click(&mut app, Position::new(area.x, area.y));
            assert!(app.focus == pane);
            app.focus = Pane::Table;
            click(&mut app, Position::new(area.x + 2, area.y + 1));
            assert!(app.focus == pane);
            app.focus = Pane::Table;
        }
        // The search box and the title are in no pane.
        app.focus = Pane::Diff;
        click(&mut app, Position::new(0, 0));
        assert!(app.focus == Pane::Diff);
    }

    fn wheel(app: &mut App, position: Position, down: bool) {
        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        app.update(Msg::Mouse(MouseEvent {
            kind: if down {
                MouseEventKind::ScrollDown
            } else {
                MouseEventKind::ScrollUp
            },
            column: position.x,
            row: position.y,
            modifiers: KeyModifiers::NONE,
        }));
    }

    /// An app whose highlighted PR has enough checks, comments, description and diff to scroll.
    fn app_with_long_panes() -> App {
        let checks = (0..40)
            .map(|i| run(&format!("check {i}"), "COMPLETED", "SUCCESS"))
            .collect();
        let mut app = app_with(vec![
            pr_checked(1, "MERGEABLE", checks),
            pr_checked(2, "MERGEABLE", vec![]),
            pr_checked(3, "MERGEABLE", vec![]),
        ]);
        let long = vec!["a line"; 100].join("\n\n");
        let comment = serde_json::json!({
            "author": {"login": "someone"},
            "createdAt": "2026-01-02T03:00:00Z",
            "body": long,
        });
        let details = serde_json::from_value(serde_json::json!({
            "headRefName": "branch",
            "body": long,
            "comments": [comment, comment, comment],
            "additions": 1,
            "deletions": 1,
        }))
        .unwrap();
        app.details_loaded(1, Ok(details));
        app.diff_loaded(1, Ok(vec!["+added"; 200].join("\n")));
        draw(&mut app, 200, 40);
        app
    }

    /// A point inside each pane.
    struct Points {
        table: Position,
        checks: Position,
        description: Position,
        diff: Position,
        comments: Position,
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

        assert!(app.focus == Pane::Table);
    }

    #[test]
    fn the_highlighted_check_is_drawn_and_goes_back_to_the_first_with_the_pr() {
        let mut app = app_with_long_panes();
        app.focus = Pane::Checks;
        press(&mut app, "jj");
        let buffer = draw(&mut app, 200, 40);
        assert_eq!(app.scroll.check, 2);
        // The first row under the header is check 0; the highlight is on the third.
        let checks = app.drawn.areas.checks;
        let reversed = |row: u16| {
            buffer[(checks.x + 2, checks.y + 2 + row)]
                .modifier
                .contains(Modifier::REVERSED)
        };
        assert!(!reversed(0) && !reversed(1) && reversed(2));

        // It stays through redraws, and goes back to the first when the PR changes.
        draw(&mut app, 200, 40);
        assert_eq!(app.scroll.check, 2);
        app.focus = Pane::Table;
        press(&mut app, "j");
        draw(&mut app, 200, 40);
        assert_eq!(app.scroll.check, 0);
    }

    #[test]
    fn a_wide_table_in_the_description_scrolls_sideways() {
        let mut app = app_with(vec![pr_checked(1, "MERGEABLE", vec![])]);
        let wide = "x".repeat(120);
        let details = serde_json::from_value(serde_json::json!({
            "headRefName": "branch",
            "body": format!("Some prose\n\n| Name | Value |\n|---|---|\n| first | {wide}END |"),
            "comments": [],
            "additions": 1,
            "deletions": 1,
        }))
        .unwrap();
        app.details_loaded(1, Ok(details));
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(screen.iter().any(|line| line.contains("scroll tables")));
        assert!(!screen.iter().any(|line| line.contains("END")));

        app.focus = Pane::Description;
        for _ in 0..20 {
            press(&mut app, "l");
        }
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(screen.iter().any(|line| line.contains("END")));
        // The prose stays where it was, and the scroll stops at the table's end.
        assert!(screen.iter().any(|line| line.contains("Some prose")));
        let at_end = app.scroll.description_x;
        press(&mut app, "l");
        draw(&mut app, 200, 40);
        assert_eq!(app.scroll.description_x, at_end);

        // And it scrolls back.
        press(&mut app, "h");
        assert!(app.scroll.description_x < at_end);
    }

    /// Details with these approvals and requests (as JSON), and nothing else.
    fn details_with_approvers(approvers: serde_json::Value) -> crate::gh::PrDetails {
        serde_json::from_value(serde_json::json!({
            "headRefName": "branch",
            "body": "",
            "comments": [],
            "additions": 1,
            "deletions": 1,
            "approvers": approvers,
        }))
        .unwrap()
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

    #[test]
    fn the_approvers_pane_grows_with_its_lines_up_to_a_limit() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let requests = |n: usize| -> serde_json::Value {
            (0..n)
                .map(
                    |i| serde_json::json!({"of": {"Person": format!("u{i}")}, "code_owner": false}),
                )
                .collect()
        };
        let mut heights = Vec::new();
        for n in [2, 4, 30] {
            app.details_loaded(
                1,
                Ok(details_with_approvers(
                    serde_json::json!({"requests": requests(n)}),
                )),
            );
            draw(&mut app, 200, 60);
            heights.push(app.drawn.areas.approvers.height);
        }
        // Two rows of border, and then a line each, until the limit.
        assert_eq!(heights, [4, 6, MAX_APPROVERS_ROWS]);
    }

    #[test]
    fn a_short_terminal_keeps_most_of_the_details_for_the_description() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let requests: serde_json::Value = (0..30)
            .map(|i| serde_json::json!({"of": {"Person": format!("u{i}")}, "code_owner": false}))
            .collect();
        app.details_loaded(
            1,
            Ok(details_with_approvers(
                serde_json::json!({"requests": requests}),
            )),
        );
        draw(&mut app, 100, 24);
        let areas = &app.drawn.areas;
        assert!(areas.description.height >= areas.approvers.height);
    }

    #[test]
    fn the_approvers_pane_marks_each_reviewer_by_how_it_stands() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let details = details_with_approvers(serde_json::json!({
            "approvals": [
                {"by": {"Person": "alice"}, "verdict": "APPROVED", "stale": false,
                 "for_teams": ["org/owners"]},
                {"by": {"Person": "bob"}, "verdict": "APPROVED", "stale": true},
                {"by": {"Person": "carol"}, "verdict": "CHANGES_REQUESTED", "stale": false},
            ],
            "requests": [
                {"of": {"Team": "org/owners"}, "code_owner": true},
                {"of": {"Person": "dave"}, "code_owner": false},
                {"of": "Hidden", "code_owner": true},
            ],
        }));
        app.details_loaded(1, Ok(details));
        let buffer = draw(&mut app, 200, 60);
        let screen = lines(&buffer);
        let has = |text: &str| screen.iter().any(|line| line.contains(text));
        assert!(has("Approvers"));
        assert!(has("✗ @carol · changes requested"));
        assert!(has("✓ @alice · approved · for org/owners"));
        assert!(has("✓ @bob · approved · on an older commit"));
        assert!(has("… org/owners · waiting · code owner"));
        assert!(has("… @dave · waiting"));
        // It is long, so it wraps in the narrow pane.
        assert!(has("… a team or reviewer"));

        // The colours are the checks pane's: red, green, and yellow for what may not count.
        let colour = |text: &str| {
            let at = find(&buffer, text).unwrap();
            buffer[at].fg
        };
        assert_eq!(colour("✗ @carol"), Color::Red);
        assert_eq!(colour("✓ @alice"), Color::Green);
        assert_eq!(colour("✓ @bob"), Color::Yellow);
        assert_eq!(colour("… @dave"), Color::Yellow);

        // Changes requested come first, then approvals, and what is still asked comes last, the
        // code owners before the rest.
        let row = |text: &str| find(&buffer, text).unwrap().y;
        assert!(row("@carol") < row("@alice") && row("@alice") < row("@bob"));
        assert!(row("@bob") < row("org/owners · waiting"));
        assert!(row("org/owners · waiting") < row("@dave"));
    }

    #[test]
    fn the_approvers_pane_says_when_there_is_nothing_and_while_it_loads() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(screen.iter().any(|line| line.contains("Loading…")));
        app.details_loaded(1, Ok(details_with_approvers(serde_json::json!({}))));
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(
            screen
                .iter()
                .any(|line| line.contains("No reviews or review requests."))
        );
        app.details_loaded(1, Err(color_eyre::eyre::eyre!("boom")));
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(
            screen
                .iter()
                .any(|line| line.contains("Failed to load details: boom"))
        );
    }

    #[test]
    fn a_long_list_of_approvers_scrolls_with_a_scrollbar() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let requests: Vec<_> = (0..30)
            .map(|i| serde_json::json!({"of": {"Person": format!("user{i}")}, "code_owner": false}))
            .collect();
        app.details_loaded(
            1,
            Ok(details_with_approvers(
                serde_json::json!({"requests": requests}),
            )),
        );
        let buffer = draw(&mut app, 200, 40);
        assert!(has_scrollbar(&buffer, app.drawn.areas.approvers));
        assert!(find(&buffer, "@user0 ").is_some() && find(&buffer, "@user29 ").is_none());
        focus(&mut app, Pane::Approvers);
        key(&mut app, KeyCode::End);
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(screen.iter().any(|line| line.contains("@user29 ")));
        assert!(!screen.iter().any(|line| line.contains("@user0 ")));
    }

    #[test]
    fn reviews_and_threads_are_headed_by_what_they_are_and_a_thread_shows_its_replies() {
        let mut app = app_with(vec![pr_checked(1, "MERGEABLE", vec![])]);
        let details = serde_json::from_value(serde_json::json!({
            "headRefName": "branch",
            "body": "",
            "comments": [
                {"author": {"login": "alice"}, "createdAt": "2026-01-02T03:00:00Z", "body": "plain"},
                {"author": {"login": "bob"}, "createdAt": "2026-01-02T04:00:00Z",
                 "body": "Looks good", "kind": {"Review": "APPROVED"}},
                {"author": {"login": "carol"}, "createdAt": "2026-01-02T05:00:00Z",
                 "body": "Why this?", "kind": {"Thread": {
                    "path": "src/main.rs", "line": 12, "resolved": true, "outdated": false}},
                 "replies": [
                    {"author": {"login": "dave"}, "createdAt": "2026-01-02T05:30:00Z",
                     "body": "Because."}]},
                {"author": {"login": "erin"}, "createdAt": "2026-01-02T06:00:00Z",
                 "body": "Old", "kind": {"Thread": {
                    "path": "src/lib.rs", "line": null, "resolved": false, "outdated": true}}},
            ],
            "additions": 1,
            "deletions": 1,
        }))
        .unwrap();
        app.details_loaded(1, Ok(details));
        app.focus = Pane::Comments;
        let screen = lines(&draw(&mut app, 200, 40));
        let has = |text: &str| screen.iter().any(|line| line.contains(text));
        assert!(has("▶ @alice · "));
        assert!(!has("@alice · approved"));
        assert!(has("▶ @bob · approved · "));
        assert!(has("▶ @carol · src/main.rs:12 · 1 reply · resolved · "));
        assert!(has("▶ @erin · src/lib.rs · outdated · "));
        assert!(!has("Because."));

        // Opening the thread shows the first comment, then each reply under who wrote it.
        press(&mut app, "jj ");
        let screen = lines(&draw(&mut app, 200, 40));
        let has = |text: &str| screen.iter().any(|line| line.contains(text));
        assert!(has("Why this?"));
        assert!(has("@dave · 2026-01-02 05:30"));
        assert!(has("Because."));
    }

    #[test]
    fn an_open_comment_is_rendered_as_markdown_and_its_table_scrolls_sideways() {
        let mut app = app_with(vec![pr_checked(1, "MERGEABLE", vec![])]);
        let wide = "x".repeat(120);
        let comment = |body: String| {
            serde_json::json!({
                "author": {"login": "someone"},
                "createdAt": "2026-01-02T03:00:00Z",
                "body": body,
            })
        };
        let details = serde_json::from_value(serde_json::json!({
            "headRefName": "branch",
            "body": "",
            "comments": [
                comment("first".to_string()),
                comment(format!("Some **bold** prose\n\n| Name | Value |\n|---|---|\n| a | {wide}END |")),
            ],
            "additions": 1,
            "deletions": 1,
        }))
        .unwrap();
        app.details_loaded(1, Ok(details));
        app.focus = Pane::Comments;
        // Collapsed, there are only the headers.
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(screen.iter().any(|line| line.contains("▶ @someone")));
        assert!(!screen.iter().any(|line| line.contains("Some bold prose")));

        press(&mut app, "j ");
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(screen.iter().any(|line| line.contains("▼ @someone")));
        assert!(screen.iter().any(|line| line.contains("Some bold prose")));
        assert!(!screen.iter().any(|line| line.contains("END")));

        for _ in 0..20 {
            press(&mut app, "l");
        }
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(screen.iter().any(|line| line.contains("END")));

        // Collapsing hides it again.
        press(&mut app, " ");
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(!screen.iter().any(|line| line.contains("Some bold prose")));
    }

    #[test]
    fn a_click_on_a_comment_header_highlights_and_toggles_it() {
        let mut app = app_with_long_panes();
        let comments = app.drawn.areas.comments;
        let header = |row: u16| Position::new(comments.x + 2, comments.y + 1 + row);

        // All collapsed: the headers are on consecutive rows.
        click(&mut app, header(1));
        assert_eq!(app.scroll.comment, 1);
        assert!(app.scroll.comments_open.contains(&1));
        assert!(app.focus == Pane::Comments);
        draw(&mut app, 200, 40);
        // The body is under the header now, and a click on it does nothing more.
        click(&mut app, header(2));
        assert_eq!(app.scroll.comment, 1);
        assert!(app.scroll.comments_open.contains(&1));
        // The border and the space below the last comment do nothing either.
        click(&mut app, Position::new(comments.x + 2, comments.y));
        assert!(app.scroll.comments_open.contains(&1));
        // A second click collapses it.
        click(&mut app, header(1));
        assert!(app.scroll.comments_open.is_empty());
    }

    #[test]
    fn a_click_on_a_header_in_a_scrolled_pane_finds_the_right_comment() {
        let mut app = app_with_long_panes();
        let comments = app.drawn.areas.comments;
        click(&mut app, Position::new(comments.x + 2, comments.y + 1));
        assert!(app.scroll.comments_open.contains(&0));
        draw(&mut app, 200, 40);
        // Scroll to the end, where the pane's scroll has been clamped, so the second comment's
        // header is some way down the pane.
        let second = app.drawn.comment_layout.items[1].0;
        app.scroll.comments = u16::MAX;
        draw(&mut app, 200, 40);
        let row = u16::try_from(second).unwrap() - app.scroll.comments;
        assert!(row > 0);
        click(
            &mut app,
            Position::new(comments.x + 2, comments.y + 1 + row),
        );
        assert_eq!(app.scroll.comment, 1);
        assert!(app.scroll.comments_open.contains(&1));
    }

    #[test]
    fn a_click_on_a_check_highlights_it_in_a_scrolled_pane() {
        let mut app = app_with_long_panes();
        let checks = app.drawn.areas.checks;
        // The first row under the header, then after scrolling the same row is a later check.
        click(&mut app, Position::new(checks.x + 2, checks.y + 3));
        assert_eq!(app.scroll.check, 1);
        app.scroll.checks = 5;
        draw(&mut app, 200, 40);
        click(&mut app, Position::new(checks.x + 2, checks.y + 2));
        assert_eq!(app.scroll.check, 5);
        // The border and the header highlight nothing.
        click(&mut app, Position::new(checks.x + 2, checks.y));
        click(&mut app, Position::new(checks.x + 2, checks.y + 1));
        assert_eq!(app.scroll.check, 5);
        assert!(app.focus == Pane::Checks);
    }

    #[test]
    fn the_wheel_is_ignored_in_a_dialog() {
        let mut app = app_with_long_panes();
        let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
        press(&mut app, "?");
        wheel(&mut app, diff, true);
        assert_eq!(app.scroll.diff, 0);
    }

    #[test]
    fn a_click_highlights_the_row_under_it_on_a_scrolled_filtered_list() {
        let prs = (1..=40)
            .map(|n| {
                pr(
                    n,
                    &format!("Bump {}", if n % 2 == 0 { "even" } else { "odd" }),
                    "OPEN",
                    "",
                )
            })
            .collect();
        let mut app = app_with(prs);
        // Only the 20 even PRs are shown; the table is too short to show them all.
        press(&mut app, "feven");
        app.update(Msg::Key(KeyCode::Enter.into()));
        draw(&mut app, 200, 40);
        let rows = app.drawn.areas.table_rows;
        click(&mut app, Position::new(rows.x + 5, rows.y + 2));
        assert_eq!(app.table.selected(), Some(2));
        assert_eq!(app.highlighted().unwrap().pr.number, 6);

        // Highlight the last row so the table scrolls, then click the first row drawn.
        for _ in 0..30 {
            press(&mut app, "j");
        }
        draw(&mut app, 200, 40);
        let offset = app.drawn.areas.table_offset;
        assert!(offset > 0);
        let rows = app.drawn.areas.table_rows;
        click(&mut app, Position::new(rows.x + 5, rows.y));
        assert_eq!(app.table.selected(), Some(offset));
        assert_eq!(
            app.highlighted().unwrap().pr.number,
            2 * (offset as u64 + 1)
        );
    }

    #[test]
    fn a_click_on_a_checkbox_toggles_it_and_highlights_the_row() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
        ]);
        draw(&mut app, 200, 30);
        let rows = app.drawn.areas.table_rows;
        // The checkbox of the second row, while the first is highlighted.
        click(&mut app, Position::new(rows.x + 1, rows.y + 1));
        assert_eq!(app.selected, HashSet::from([2]));
        assert_eq!(app.table.selected(), Some(1));
        let buffer = draw(&mut app, 200, 30);
        assert!(
            lines(&buffer)
                .iter()
                .any(|l| l.contains("[x]") && l.contains("#2"))
        );
        click(&mut app, Position::new(rows.x + 1, rows.y + 1));
        assert!(app.selected.is_empty());

        // Just beside the checkbox is the row itself: it highlights and selects nothing.
        click(&mut app, Position::new(rows.x + 3, rows.y));
        assert!(app.selected.is_empty());
        assert_eq!(app.table.selected(), Some(0));

        // Space acts on the same selection, and a checkbox click adds to it.
        press(&mut app, " ");
        click(&mut app, Position::new(rows.x + 1, rows.y + 1));
        assert_eq!(app.selected, HashSet::from([1, 2]));
    }

    #[test]
    fn a_click_on_the_header_or_below_the_rows_does_nothing() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
        ]);
        draw(&mut app, 200, 30);
        let rows = app.drawn.areas.table_rows;
        click(&mut app, Position::new(rows.x + 5, rows.y + 1));
        assert_eq!(app.table.selected(), Some(1));
        click(&mut app, Position::new(rows.x + 5, rows.y - 1));
        click(&mut app, Position::new(rows.x + 5, rows.y + 5));
        assert_eq!(app.table.selected(), Some(1));
    }

    #[test]
    fn the_mouse_is_ignored_in_a_dialog() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw(&mut app, 200, 40);
        let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
        press(&mut app, "?");
        click(&mut app, diff);
        assert!(app.focus == Pane::Table);
        // Not even a box's area is live behind a dialog.
        click(&mut app, Position::new(2, 2));
        assert!(app.typing == Typing::Nothing);
    }

    /// The centre of the dialog's `index`th button, as last drawn.
    fn button(app: &App, index: usize) -> Position {
        let area = app.drawn.buttons[index];
        Position::new(area.x + area.width / 2, area.y)
    }

    fn click_button(app: &mut App, index: usize) {
        let position = button(app, index);
        click(app, position);
    }

    /// An app with the confirm dialog open for two PRs, drawn.
    fn app_confirming() -> App {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
        ]);
        press(&mut app, " j ");
        ctrl_g(&mut app);
        assert!(matches!(app.mode, Mode::Confirm(_)));
        draw(&mut app, 200, 40);
        app
    }

    /// An app with the dialog that asks before opening six PRs open, drawn.
    fn app_confirming_open() -> App {
        let mut app = app_with((1..=6).map(|n| pr(n, "Bump", "OPEN", "")).collect());
        for _ in 0..6 {
            press(&mut app, " j");
        }
        app.update(Msg::Key(crossterm::event::KeyEvent::new(
            KeyCode::Char('o'),
            crossterm::event::KeyModifiers::CONTROL,
        )));
        assert!(matches!(app.mode, Mode::ConfirmOpen(_)));
        draw(&mut app, 200, 40);
        app
    }

    #[test]
    fn the_buttons_are_recorded_where_they_are_drawn() {
        let mut app = app_confirming();
        let buffer = draw(&mut app, 200, 40);
        let drawn: Vec<String> = app
            .drawn
            .buttons
            .iter()
            .map(|area| {
                (area.x..area.right())
                    .map(|x| buffer[(x, area.y)].symbol())
                    .collect()
            })
            .collect();
        assert_eq!(drawn, ["[ Approve ]", "[ Approve & merge ]", "[ Cancel ]"]);
    }

    #[test]
    fn clicking_approve_or_approve_and_merge_starts_the_batch() {
        for (index, merge) in [(0, false), (1, true)] {
            let mut app = app_confirming();
            click_button(&mut app, index);
            let Mode::Running(batch) = &app.mode else {
                panic!("button {index} did not start the batch");
            };
            assert_eq!(batch.merge, merge);
            assert_eq!(app.approvals_to_run(), [1, 2]);
        }
    }

    #[test]
    fn clicking_cancel_closes_the_confirm_dialog() {
        let mut app = app_confirming();
        click_button(&mut app, 2);
        assert!(matches!(app.mode, Mode::List));
        assert!(app.approvals_to_run().is_empty());
    }

    #[test]
    fn clicking_open_or_cancel_answers_the_open_dialog() {
        let mut app = app_confirming_open();
        click_button(&mut app, 0);
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.opens_to_run(), [1, 2, 3, 4, 5, 6]);

        let mut app = app_confirming_open();
        click_button(&mut app, 1);
        assert!(matches!(app.mode, Mode::List));
        assert!(app.opens_to_run().is_empty());
    }

    #[test]
    fn clicking_ok_closes_the_help_and_results_popups() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "?");
        draw(&mut app, 200, 40);
        assert_eq!(app.drawn.buttons.len(), 1);
        click_button(&mut app, 0);
        assert!(matches!(app.mode, Mode::List));

        let mut app = app_confirming();
        click_button(&mut app, 0);
        // `q` cancels the running batch, which shows the results.
        press(&mut app, "q");
        assert!(matches!(app.mode, Mode::Results(_)));
        draw(&mut app, 200, 40);
        click_button(&mut app, 0);
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn a_click_on_the_dialog_but_not_a_button_or_outside_it_does_nothing() {
        let mut app = app_confirming();
        let first = app.drawn.buttons[0];
        // The row above the buttons is inside the dialog; (0, 0) is outside it, on the title.
        for position in [Position::new(first.x, first.y - 1), Position::new(0, 0)] {
            click(&mut app, position);
            let Mode::Confirm(confirm) = &app.mode else {
                panic!("the dialog closed");
            };
            assert_eq!(confirm.choice, 0);
        }
        // Between two buttons.
        let gap = app.drawn.buttons[0].right();
        click(&mut app, Position::new(gap, first.y));
        assert!(matches!(app.mode, Mode::Confirm(_)));
        // Nor does a click behind it reach the panes.
        let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
        click(&mut app, diff);
        assert!(app.focus == Pane::Table);
    }

    #[test]
    fn the_wheel_over_a_button_does_nothing() {
        let mut app = app_confirming();
        let position = button(&app, 0);
        wheel(&mut app, position, true);
        assert!(matches!(app.mode, Mode::Confirm(_)));
    }

    #[test]
    fn a_click_does_nothing_while_a_batch_runs() {
        let mut app = app_confirming();
        click_button(&mut app, 0);
        draw(&mut app, 200, 40);
        assert!(app.drawn.buttons.is_empty());
        for position in [Position::new(80, 20), Position::new(0, 0)] {
            click(&mut app, position);
            assert!(matches!(app.mode, Mode::Running(_)));
        }
    }

    #[test]
    fn no_buttons_are_recorded_once_the_dialog_closes() {
        let mut app = app_confirming();
        assert!(!app.drawn.buttons.is_empty());
        press(&mut app, "q");
        draw(&mut app, 200, 40);
        assert!(app.drawn.buttons.is_empty());
    }

    /// Whether `area`'s right border has a scrollbar (its track or thumb) beside the first row.
    fn has_scrollbar(buffer: &Buffer, area: Rect) -> bool {
        matches!(buffer[(area.right() - 1, area.y + 1)].symbol(), "█" | "║")
    }

    #[test]
    fn a_pane_with_too_much_text_has_a_scrollbar() {
        let mut app = app_with_long_panes();
        // Open the first comment, whose body is long.
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        press(&mut app, " ");
        let buffer = draw(&mut app, 200, 40);
        let areas = &app.drawn.areas;
        for (name, area) in [
            ("checks", areas.checks),
            ("comments", areas.comments),
            ("description", areas.description),
            ("diff", areas.diff),
        ] {
            assert!(has_scrollbar(&buffer, area), "{name}");
        }
    }

    #[test]
    fn a_pane_whose_text_fits_has_no_scrollbar() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let buffer = draw(&mut app, 200, 40);
        let areas = &app.drawn.areas;
        for (name, area) in [
            ("table", areas.table),
            ("checks", areas.checks),
            ("comments", areas.comments),
            ("description", areas.description),
            ("diff", areas.diff),
        ] {
            assert!(!has_scrollbar(&buffer, area), "{name}");
        }
    }

    #[test]
    fn the_table_has_a_scrollbar_only_with_more_rows_than_fit() {
        let mut app = app_with((1..=80).map(|n| pr(n, "Bump", "OPEN", "")).collect());
        let buffer = draw(&mut app, 200, 40);
        assert!(has_scrollbar(&buffer, app.drawn.areas.table));

        press(&mut app, "f#80");
        let buffer = draw(&mut app, 200, 40);
        assert!(!has_scrollbar(&buffer, app.drawn.areas.table));
    }

    #[test]
    fn the_scrollbar_thumb_moves_to_the_bottom_with_the_scroll() {
        let mut app = app_with_long_panes();
        let thumb_rows = |buffer: &Buffer, area: Rect| -> Vec<u16> {
            (area.y + 1..area.bottom() - 1)
                .filter(|&y| buffer[(area.right() - 1, y)].symbol() == "█")
                .collect()
        };
        let buffer = draw(&mut app, 200, 40);
        let diff = app.drawn.areas.diff;
        let at_top = thumb_rows(&buffer, diff);
        assert_eq!(at_top.first(), Some(&(diff.y + 1)));
        assert!(at_top.last() < Some(&(diff.bottom() - 2)));

        for _ in 0..5 {
            key(&mut app, KeyCode::Tab);
        }
        key(&mut app, KeyCode::End);
        let buffer = draw(&mut app, 200, 40);
        let at_bottom = thumb_rows(&buffer, diff);
        assert_eq!(at_bottom.last(), Some(&(diff.bottom() - 2)));
        assert!(at_bottom.first() > Some(&(diff.y + 1)));
    }

    #[test]
    fn the_search_and_filter_boxes_colour_only_their_border_and_title_while_edited() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "fbump");
        key(&mut app, KeyCode::Enter);
        let buffer = draw(&mut app, 200, 40);
        let search = app.drawn.areas.search;
        let filter = app.drawn.areas.filter;
        // Not being edited: nothing is cyan.
        assert_ne!(buffer[(search.x, search.y + 1)].fg, Color::Cyan);
        assert_ne!(buffer[(filter.x, filter.y + 1)].fg, Color::Cyan);

        press(&mut app, "/");
        let buffer = draw(&mut app, 200, 40);
        assert_eq!(buffer[(search.x, search.y + 1)].fg, Color::Cyan);
        assert_eq!(buffer[(search.x + 1, search.y)].fg, Color::Cyan);
        assert_ne!(buffer[(search.x + 1, search.y + 1)].fg, Color::Cyan);
        assert_ne!(buffer[(filter.x, filter.y + 1)].fg, Color::Cyan);

        key(&mut app, KeyCode::Esc);
        press(&mut app, "f");
        let buffer = draw(&mut app, 200, 40);
        assert_eq!(buffer[(filter.x, filter.y + 1)].fg, Color::Cyan);
        assert_ne!(buffer[(filter.x + 1, filter.y + 1)].fg, Color::Cyan);
        assert_ne!(buffer[(search.x, search.y + 1)].fg, Color::Cyan);
    }

    #[test]
    fn the_suggestions_are_listed_under_the_word_with_the_chosen_one_reversed() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "/ la");
        let buffer = draw(&mut app, 200, 40);
        let search = app.drawn.areas.search;
        let label = find(&buffer, " label ").unwrap();
        // Inside the box's border, which starts under the word: "is:pr " is six columns.
        assert_eq!(label.y, search.bottom() + 1);
        assert_eq!(label.x, search.x + 1 + 6 + 1);
        let language = find(&buffer, " language ").unwrap();
        assert_eq!(language.y, label.y + 1);
        // Nothing is chosen to begin with, and the hints say Enter searches.
        assert!(!buffer[label].modifier.contains(Modifier::REVERSED));
        assert!(!buffer[language].modifier.contains(Modifier::REVERSED));
        let status = lines(&buffer).pop().unwrap();
        assert!(status.contains("Enter: Search | Tab/Up/Down: Choose"));

        key(&mut app, KeyCode::Tab);
        let buffer = draw(&mut app, 200, 40);
        assert!(buffer[label].modifier.contains(Modifier::REVERSED));
        assert!(!buffer[language].modifier.contains(Modifier::REVERSED));
        // Now the hints say Enter takes it.
        assert!(lines(&buffer).pop().unwrap().contains("Enter: Take"));

        key(&mut app, KeyCode::Tab);
        let buffer = draw(&mut app, 200, 40);
        assert!(!buffer[label].modifier.contains(Modifier::REVERSED));
        assert!(buffer[language].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn many_suggestions_scroll_to_keep_the_highlight_in_view() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "/ sort:");
        for _ in 0..9 {
            key(&mut app, KeyCode::Down);
        }
        let buffer = draw(&mut app, 200, 40);
        let screen = lines(&buffer);
        // Ten values, eight rows: the ninth is highlighted at the bottom and the first has gone.
        assert!(find(&buffer, " interactions-desc ").is_some());
        assert!(!screen.iter().any(|l| l.contains(" created-desc ")));
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

    #[test]
    fn a_drag_does_nothing() {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
        ]);
        draw(&mut app, 200, 40);
        let rows = app.drawn.areas.table_rows;
        let diff = app.drawn.areas.diff;
        let drag = |app: &mut App, x: u16, y: u16| {
            app.update(Msg::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            }));
        };
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
        assert!(app.focus == Pane::Table);
    }

    #[test]
    fn a_wide_terminal_puts_the_list_beside_the_details() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 200, 40);
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
    fn a_narrow_terminal_gives_the_list_the_full_width_above_everything_else() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 120, 40);
        let areas = &app.drawn.areas;
        assert_eq!((areas.table.x, areas.table.width), (0, 120));
        for (name, area) in [
            ("checks", areas.checks),
            ("comments", areas.comments),
            ("description", areas.description),
            ("diff", areas.diff),
        ] {
            assert!(area.y >= areas.table.bottom(), "{name} is below the list");
        }
    }

    #[test]
    fn a_narrow_terminal_puts_the_description_beside_the_diff_above_the_checks_and_comments() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 120, 40);
        let areas = &app.drawn.areas;
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
            assert_eq!(app.drawn.areas.table.width < width, wide, "{width}");
        }
    }

    #[test]
    fn a_narrow_terminal_shows_the_author_and_title_in_the_list() {
        let mut app = app_with(vec![by("app/renovate", 1, "Bump alpha")]);
        let screen = lines(&draw(&mut app, 120, 40));
        let row = screen
            .iter()
            .find(|l| l.contains("[ ]") && l.contains("#1"))
            .unwrap();
        assert!(row.contains("app/renovate") && row.contains("Bump alpha"));
    }

    #[test]
    fn every_pane_is_drawn_and_clickable_in_a_narrow_terminal() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        draw(&mut app, 100, 40);
        for pane in [
            Pane::Checks,
            Pane::Approvers,
            Pane::Comments,
            Pane::Description,
            Pane::Diff,
            Pane::Table,
        ] {
            let area = match pane {
                Pane::Table => app.drawn.areas.table,
                Pane::Checks => app.drawn.areas.checks,
                Pane::Approvers => app.drawn.areas.approvers,
                Pane::Comments => app.drawn.areas.comments,
                Pane::Description => app.drawn.areas.description,
                Pane::Diff => app.drawn.areas.diff,
            };
            assert!(area.width > 2 && area.height > 2, "{pane:?}");
            click(&mut app, Position::new(area.x + 1, area.y + 1));
            assert!(app.focus == pane, "{pane:?}");
        }
    }

    /// The text of row `row` (counted from the first inside the border) of the pane at `area`.
    fn pane_row(buffer: &Buffer, area: Rect, row: u16) -> String {
        (area.x + 1..area.right() - 1)
            .map(|x| buffer[(x, area.y + 1 + row)].symbol())
            .collect()
    }

    fn focus(app: &mut App, pane: Pane) {
        while app.focus != pane {
            key(app, KeyCode::Tab);
        }
    }

    #[test]
    fn the_diff_scrolls_sideways_to_the_end_of_its_longest_line() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.diff_loaded(1, Ok(format!("+{}END\n ctx", "x".repeat(300))));
        let buffer = draw(&mut app, 120, 40);
        let diff = app.drawn.areas.diff;
        assert!(pane_row(&buffer, diff, 0).starts_with("+xxx"));
        assert!(!pane_row(&buffer, diff, 0).contains("END"));

        focus(&mut app, Pane::Diff);
        for _ in 0..60 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw(&mut app, 120, 40);
        // Stopped at the end of the line, not scrolled on into blank space.
        assert!(pane_row(&buffer, diff, 0).trim_end().ends_with("xxxEND"));
        assert!(pane_row(&buffer, diff, 0).starts_with("xxx"));
        assert_eq!(pane_row(&buffer, diff, 0).chars().last(), Some('D'));

        for _ in 0..60 {
            key(&mut app, KeyCode::Left);
        }
        let buffer = draw(&mut app, 120, 40);
        assert!(pane_row(&buffer, diff, 0).starts_with("+xxx"));
    }

    #[test]
    fn the_checks_scroll_their_names_sideways_and_keep_the_status() {
        let name = format!("{}END", "n".repeat(120));
        let mut app = app_with(vec![pr_checked(
            1,
            "MERGEABLE",
            vec![run(&name, "COMPLETED", "SUCCESS")],
        )]);
        draw(&mut app, 120, 40);
        focus(&mut app, Pane::Checks);
        let buffer = draw(&mut app, 120, 40);
        let checks = app.drawn.areas.checks;
        assert!(pane_row(&buffer, checks, 1).contains("passing"));
        assert!(!pane_row(&buffer, checks, 1).contains("END"));

        for _ in 0..30 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw(&mut app, 120, 40);
        let row = pane_row(&buffer, checks, 1);
        assert!(row.contains("passing"), "the status stays: {row}");
        assert!(row.trim_end().ends_with("END"), "{row}");
        assert!(!row.contains("CI/"), "{row}");
    }

    fn app_with_a_long_title() -> App {
        let title = format!("{}END", "long ".repeat(40));
        app_with(vec![by("alice", 1, &title), by("bob", 2, "Short")])
    }

    #[test]
    fn the_list_scrolls_sideways_keeping_the_checkbox_and_number() {
        let mut app = app_with_a_long_title();
        let buffer = draw(&mut app, 100, 40);
        let table = app.drawn.areas.table;
        assert!(!pane_row(&buffer, table, 1).contains("END"));

        for _ in 0..40 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw(&mut app, 100, 40);
        let row = pane_row(&buffer, table, 1);
        assert!(row.starts_with("[ ] #1 "), "the first columns stay: {row}");
        assert!(row.trim_end().ends_with("END"), "{row}");
        assert!(!row.contains("alice"), "the author scrolled away: {row}");
        // The header scrolls with the rows, and keeps the number's.
        assert!(pane_row(&buffer, table, 0).trim_start().starts_with('#'));
        assert!(!pane_row(&buffer, table, 0).contains("Author"));
        // The second PR's row is still there under its number.
        assert!(pane_row(&buffer, table, 2).starts_with("[ ] #2 "));

        for _ in 0..40 {
            key(&mut app, KeyCode::Left);
        }
        let buffer = draw(&mut app, 100, 40);
        assert!(pane_row(&buffer, table, 1).contains("alice"));
    }

    #[test]
    fn the_lists_scroll_stays_when_the_highlight_moves() {
        let mut app = app_with_a_long_title();
        draw(&mut app, 100, 40);
        for _ in 0..3 {
            key(&mut app, KeyCode::Right);
        }
        let x = app.list_x;
        assert!(x > 0);
        press(&mut app, "j");
        draw(&mut app, 100, 40);
        assert_eq!(app.list_x, x);
    }

    #[test]
    fn a_list_that_fits_does_not_scroll_sideways() {
        let mut app = app_with(vec![by("alice", 1, "Bump alpha")]);
        for _ in 0..5 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw(&mut app, 100, 40);
        assert_eq!(app.list_x, 0);
        assert!(pane_row(&buffer, app.drawn.areas.table, 1).contains("alice"));
    }

    #[test]
    fn the_wheel_scrolls_the_list_and_the_diff_sideways() {
        use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        let mut app = app_with_a_long_title();
        app.diff_loaded(1, Ok(format!("+{}END", "x".repeat(300))));
        draw(&mut app, 100, 40);
        let table = app.drawn.areas.table;
        let diff = app.drawn.areas.diff;
        let scroll = |app: &mut App, kind, modifiers, area: Rect| {
            app.update(Msg::Mouse(MouseEvent {
                kind,
                column: area.x + 2,
                row: area.y + 2,
                modifiers,
            }));
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
        assert!(app.focus == Pane::Table);
    }

    /// The columns of the thumb of the scrollbar on `area`'s bottom border. Its track is the
    /// border itself, so a pane has a scrollbar when it has a thumb.
    fn horizontal_thumb(buffer: &Buffer, area: Rect) -> Vec<u16> {
        (area.x + 1..area.right() - 1)
            .filter(|&x| buffer[(x, area.bottom() - 1)].symbol() == HORIZONTAL_THUMB)
            .collect()
    }

    fn has_horizontal_scrollbar(buffer: &Buffer, area: Rect) -> bool {
        !horizontal_thumb(buffer, area).is_empty()
    }

    #[test]
    fn the_list_checks_and_diff_have_a_horizontal_scrollbar_only_when_cut_off() {
        let title = "long ".repeat(40);
        let mut app = app_with(vec![by("alice", 1, &title), by("bob", 2, "Short")]);
        app.diff_loaded(1, Ok(format!("+{}", "x".repeat(300))));
        app.diff_loaded(2, Ok("+short".to_string()));
        let buffer = draw(&mut app, 100, 40);
        let areas = &app.drawn.areas;
        assert!(has_horizontal_scrollbar(&buffer, areas.table));
        assert!(has_horizontal_scrollbar(&buffer, areas.diff));
        // Its checks are all short.
        assert!(!has_horizontal_scrollbar(&buffer, areas.checks));

        // The second PR's diff fits; the list still has the first PR's long title.
        press(&mut app, "j");
        let buffer = draw(&mut app, 100, 40);
        assert!(!has_horizontal_scrollbar(&buffer, app.drawn.areas.diff));
        assert!(has_horizontal_scrollbar(&buffer, app.drawn.areas.table));
    }

    #[test]
    fn a_long_check_name_gives_the_checks_a_horizontal_scrollbar() {
        let name = "n".repeat(150);
        let mut app = app_with(vec![pr_checked(
            1,
            "MERGEABLE",
            vec![run(&name, "COMPLETED", "SUCCESS")],
        )]);
        let buffer = draw(&mut app, 100, 40);
        assert!(has_horizontal_scrollbar(&buffer, app.drawn.areas.checks));
    }

    #[test]
    fn no_pane_has_a_horizontal_scrollbar_when_its_text_fits() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.diff_loaded(1, Ok("+added".to_string()));
        let buffer = draw(&mut app, 100, 40);
        let areas = &app.drawn.areas;
        for (name, area) in [
            ("table", areas.table),
            ("checks", areas.checks),
            ("description", areas.description),
            ("diff", areas.diff),
        ] {
            assert!(!has_horizontal_scrollbar(&buffer, area), "{name}");
        }
    }

    #[test]
    fn the_horizontal_thumb_moves_from_the_left_end_to_the_right_end() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.diff_loaded(1, Ok(format!("+{}", "x".repeat(300))));
        let buffer = draw(&mut app, 100, 40);
        let diff = app.drawn.areas.diff;
        let at_start = horizontal_thumb(&buffer, diff);
        assert_eq!(at_start.first(), Some(&(diff.x + 1)));
        assert!(at_start.last() < Some(&(diff.right() - 2)));

        focus(&mut app, Pane::Diff);
        for _ in 0..60 {
            key(&mut app, KeyCode::Right);
        }
        let buffer = draw(&mut app, 100, 40);
        let at_end = horizontal_thumb(&buffer, diff);
        assert_eq!(at_end.last(), Some(&(diff.right() - 2)));
        assert!(at_end.first() > Some(&(diff.x + 1)));
    }

    /// An app whose first PR has this description and, if given, one comment with this body.
    fn app_with_bodies(body: &str, comment: Option<&str>) -> App {
        let comments: Vec<serde_json::Value> = comment
            .map(|body| {
                serde_json::json!({
                    "author": {"login": "someone"},
                    "createdAt": "2026-01-02T03:00:00Z",
                    "body": body,
                    "url": "https://example.test/c",
                })
            })
            .into_iter()
            .collect();
        let details = serde_json::from_value(serde_json::json!({
            "headRefName": "branch",
            "body": body,
            "comments": comments,
            "additions": 1,
            "deletions": 1,
        }))
        .unwrap();
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.details_loaded(1, Ok(details));
        app
    }

    /// Clicks the first place `text` is drawn on screen, and returns the pages queued.
    fn click_text(app: &mut App, width: u16, text: &str) -> Vec<String> {
        let buffer = draw(app, width, 40);
        let position = find(&buffer, text).unwrap_or_else(|| panic!("{text} is not drawn"));
        click(app, position);
        app.pages_to_run()
    }

    #[test]
    fn a_click_on_a_link_in_the_description_opens_it_and_focuses_the_pane() {
        for text in ["the notes", "a.example/n"] {
            let mut app = app_with_bodies("Read [the notes](https://a.example/n) please.", None);
            assert_eq!(
                click_text(&mut app, 200, text),
                ["https://a.example/n"],
                "{text}"
            );
            assert!(app.focus == Pane::Description);
        }
    }

    #[test]
    fn a_click_on_a_bare_url_opens_it() {
        let mut app = app_with_bodies("See https://a.example/bare, thanks.", None);
        assert_eq!(
            click_text(&mut app, 200, "a.example/bare"),
            ["https://a.example/bare"]
        );
    }

    #[test]
    fn a_click_on_text_that_is_not_a_link_does_nothing() {
        let mut app = app_with_bodies("Read [the notes](https://a.example/n) please.", None);
        assert!(click_text(&mut app, 200, "please").is_empty());
        assert!(app.errors.text().is_none());
    }

    #[test]
    fn a_link_is_found_in_a_scrolled_description() {
        let body = format!(
            "{}\n\n[far down](https://a.example/far)",
            vec!["line"; 80].join("\n\n")
        );
        let mut app = app_with_bodies(&body, None);
        draw(&mut app, 200, 40);
        focus(&mut app, Pane::Description);
        key(&mut app, KeyCode::End);
        assert_eq!(
            click_text(&mut app, 200, "far down"),
            ["https://a.example/far"]
        );
    }

    #[test]
    fn a_url_that_wraps_over_several_rows_opens_from_any_of_them() {
        let url = "https://a.example/a/very/long/path/that/cannot/fit/on/one/row";
        let mut app = app_with_bodies(&format!("[{url}]({url})"), None);
        // The pane is under 60 columns wide.
        let buffer = draw(&mut app, 100, 40);
        let description = app.drawn.areas.description;
        assert!(find(&buffer, url).is_none(), "it is wrapped");
        // Every row inside the pane that has text is the link.
        let mut clicked = 0;
        for row in 1..description.height - 1 {
            let text = pane_row(&buffer, description, row - 1);
            if text.trim().is_empty() {
                continue;
            }
            click(
                &mut app,
                Position::new(description.x + 2, description.y + row),
            );
            assert_eq!(app.pages_to_run(), [url], "row {row}: {text}");
            clicked += 1;
        }
        assert!(clicked > 2);
    }

    #[test]
    fn a_link_in_a_table_still_opens_when_the_table_is_scrolled_sideways() {
        let body = "| Package | Change |\n|---|---|\n| [lib](https://a.example/l) | a long cell to make the table wider than the pane it is in |";
        let mut app = app_with_bodies(body, None);
        draw(&mut app, 100, 40);
        focus(&mut app, Pane::Description);
        assert_eq!(click_text(&mut app, 100, "lib"), ["https://a.example/l"]);
        key(&mut app, KeyCode::Right);
        draw(&mut app, 100, 40);
        assert!(app.scroll.description_x > 0);
        assert_eq!(
            click_text(&mut app, 100, "a.example/l"),
            ["https://a.example/l"]
        );
    }

    #[test]
    fn a_click_on_a_link_in_an_open_comment_opens_it_without_toggling_the_comment() {
        let mut app = app_with_bodies("desc", Some("Look at [this](https://a.example/c1) one."));
        draw(&mut app, 200, 40);
        focus(&mut app, Pane::Comments);
        press(&mut app, " ");
        assert_eq!(click_text(&mut app, 200, "this"), ["https://a.example/c1"]);
        assert!(app.scroll.comments_open.contains(&0), "still open");
    }

    #[test]
    fn a_click_on_a_comments_header_still_toggles_it_and_a_collapsed_comment_has_no_links() {
        let mut app = app_with_bodies("desc", Some("Look at [this](https://a.example/c1) one."));
        let buffer = draw(&mut app, 200, 40);
        assert!(find(&buffer, "this").is_none());
        let header = find(&buffer, "@someone").unwrap();
        click(&mut app, header);
        assert!(app.scroll.comments_open.contains(&0));
        assert!(app.pages_to_run().is_empty());
    }

    #[test]
    fn a_link_that_is_not_http_or_https_is_not_opened_and_says_why() {
        for url in [
            "file:///etc/passwd",
            "FILE:///etc/passwd",
            "vscode://open/x",
            "javascript:alert(1)",
            "../relative",
            "mailto:a@b.example",
        ] {
            let mut app = app_with_bodies(&format!("[local thing]({url}) here"), None);
            assert!(click_text(&mut app, 200, "local thing").is_empty(), "{url}");
            let error = app.errors.text().unwrap_or_default();
            assert!(error.contains("only http and https"), "{url}: {error}");
        }
        // Whatever the case of the scheme, a web link is fine.
        let mut app = app_with_bodies("[the notes](HTTPS://a.example/n)", None);
        assert_eq!(
            click_text(&mut app, 200, "the notes"),
            ["HTTPS://a.example/n"]
        );
    }

    #[test]
    fn clicks_do_nothing_with_links_when_the_details_have_not_loaded() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        let buffer = draw(&mut app, 200, 40);
        let description = app.drawn.areas.description;
        assert!(find(&buffer, "Loading").is_some());
        click(
            &mut app,
            Position::new(description.x + 2, description.y + 1),
        );
        assert!(app.pages_to_run().is_empty());
    }
    #[test]
    fn the_wheel_is_ignored_while_a_box_is_edited() {
        let mut app = app_with_long_panes();
        draw(&mut app, 200, 40);
        let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
        press(&mut app, "f");
        wheel(&mut app, diff, true);
        assert_eq!(app.scroll.diff, 0);
    }

    fn key(app: &mut App, code: KeyCode) {
        app.update(Msg::Key(code.into()));
    }

    /// Clicks `column` characters into the text of the search box, as last drawn.
    fn click_search(app: &mut App, column: u16) {
        let area = app.drawn.areas.search;
        click(app, Position::new(area.x + 1 + column, area.y + 1));
    }

    fn click_filter(app: &mut App, column: u16) {
        let area = app.drawn.areas.filter;
        click(app, Position::new(area.x + 1 + column, area.y + 1));
    }

    #[test]
    fn a_click_on_the_search_box_edits_the_query_with_the_cursor_there() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw(&mut app, 200, 40);
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
        draw(&mut app, 200, 40);
        assert_eq!(app.drawn.areas.filter, Rect::default());
        click(&mut app, Position::new(2, 4));
        assert!(app.typing != Typing::Filter);

        press(&mut app, "fbump");
        key(&mut app, KeyCode::Enter);
        draw(&mut app, 200, 40);
        assert!(app.typing == Typing::Nothing);
        click_filter(&mut app, 2);
        assert!(app.typing == Typing::Filter);
        assert_eq!(app.filter.cursor(), 2);
    }

    #[test]
    fn the_filter_box_has_no_area_once_it_is_emptied() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "fbump");
        draw(&mut app, 200, 40);
        assert!(app.drawn.areas.filter != Rect::default());
        key(&mut app, KeyCode::Esc);
        draw(&mut app, 200, 40);
        assert_eq!(app.drawn.areas.filter, Rect::default());
    }

    #[test]
    fn a_click_on_the_other_box_switches_to_it() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "fbump");
        draw(&mut app, 200, 40);
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
        draw(&mut app, 200, 40);
        press(&mut app, "/");
        press(&mut app, " label:deps");
        let rows = app.drawn.areas.table_rows;
        click(&mut app, Position::new(rows.x + 20, rows.y + 1));

        assert!(app.typing == Typing::Nothing);
        assert_eq!(app.query.value(), "is:pr label:deps");
        assert!(app.list_to_fetch().is_none(), "not submitted");
        // The click did what it does anywhere else.
        assert_eq!(app.highlighted().unwrap().pr.number, 2);
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(screen.iter().any(|l| l.contains("not searched")));

        // `/` resumes the same text, and Esc restores the searched query.
        press(&mut app, "/");
        assert_eq!(app.query.value(), "is:pr label:deps");
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.query.value(), "is:pr");
        let screen = lines(&draw(&mut app, 200, 40));
        assert!(!screen.iter().any(|l| l.contains("not searched")));
    }

    #[test]
    fn a_click_on_a_pane_leaves_the_filter_box_keeping_its_filter() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Update beta", "OPEN", ""),
        ]);
        press(&mut app, "fbump");
        draw(&mut app, 200, 40);
        let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
        click(&mut app, diff);

        assert!(app.typing == Typing::Nothing);
        assert!(app.focus == Pane::Diff);
        assert_eq!(app.filter.value(), "bump");
        assert_eq!(app.visible_rows().count(), 1);
    }

    #[test]
    fn ctrl_r_runs_the_submitted_query_not_the_unsearched_text() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw(&mut app, 200, 40);
        press(&mut app, "/");
        press(&mut app, " label:deps");
        click(&mut app, Position::new(2, 38));
        app.update(Msg::Key(crossterm::event::KeyEvent::new(
            KeyCode::Char('r'),
            crossterm::event::KeyModifiers::CONTROL,
        )));
        let (_, query) = app.list_to_fetch().unwrap();
        assert_eq!(query, "is:pr");
    }

    #[test]
    fn says_when_the_filter_hides_every_pr() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "fzzz");
        let screen = lines(&draw(&mut app, 200, 30));
        assert!(
            screen
                .iter()
                .any(|line| line.contains("No PRs match the filter."))
        );
    }

    #[test]
    fn the_status_bar_shows_the_focused_panes_keys() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        for expected in [
            "Enter: Review | o: Open | y: Copy | ?: Help",
            "Enter, o: Open | y: Copy | ?: Help",
            "Enter, Space: Expand | o: Open | y: Copy | ?: Help",
            "?: Help | /: Search",
            "?: Help | /: Search",
            "?: Help | /: Search",
        ] {
            let screen = lines(&draw(&mut app, 200, 30));
            let bar = screen.last().unwrap();
            assert!(bar.contains(expected), "{:?} in {bar:?}", app.focus);
            // The keys of the other panes are not on it.
            assert_eq!(
                bar.matches("o: Open").count(),
                expected.matches("o: Open").count()
            );
            app.update(Msg::Key(KeyCode::Tab.into()));
        }
    }

    #[test]
    fn the_help_popup_scrolls_on_a_short_screen_with_its_button_pinned() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "?");
        let screen = lines(&draw(&mut app, 200, 14)).join("\n");
        // Too short for every key: the start is there, the end is not, and the bar says so.
        assert!(screen.contains("Everywhere"), "{screen}");
        assert!(
            !screen.contains("Scroll the description's tables sideways"),
            "{screen}"
        );
        assert!(screen.contains("OK"), "{screen}");
        assert!(app.drawn.popup.content > app.drawn.popup.viewport);
        let bar = |app: &mut App| -> usize {
            let buffer = draw(app, 200, 14);
            let area = buffer.area;
            (0..area.height)
                .flat_map(|y| (0..area.width).map(move |x| (x, y)))
                .filter(|&pos| buffer[pos].symbol() == "█")
                .count()
        };
        assert!(bar(&mut app) > 0, "no scrollbar");

        // End shows the last key, and the button is still there.
        key(&mut app, KeyCode::End);
        let screen = lines(&draw(&mut app, 200, 14)).join("\n");
        assert!(
            screen.contains("Scroll the description's tables sideways"),
            "{screen}"
        );
        assert!(!screen.contains("Everywhere"), "{screen}");
        assert!(screen.contains("OK"), "{screen}");
        // Further down does nothing, and Home, PgDn and PgUp, Up and Down and the wheel move it.
        key(&mut app, KeyCode::Down);
        assert!(matches!(app.mode, Mode::Help(at) if at == app.drawn.popup.offset));
        key(&mut app, KeyCode::Home);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, 0);
        key(&mut app, KeyCode::PageDown);
        draw(&mut app, 200, 14);
        let page = app.drawn.popup.offset;
        assert_eq!(page, app.drawn.popup.viewport - 1);
        key(&mut app, KeyCode::Up);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, page - 1);
        key(&mut app, KeyCode::PageUp);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, 0);
        wheel(&mut app, Position::new(100, 5), true);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, 3);
        wheel(&mut app, Position::new(100, 5), false);
        draw(&mut app, 200, 14);
        assert_eq!(app.drawn.popup.offset, 0);
        // The popup still closes.
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn a_tall_help_popup_has_no_scrollbar() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "?");
        draw(&mut app, 200, 90);
        assert_eq!(app.drawn.popup.content, app.drawn.popup.viewport);
        key(&mut app, KeyCode::End);
        draw(&mut app, 200, 90);
        assert_eq!(app.drawn.popup.offset, 0);
    }

    #[test]
    fn the_help_popup_groups_the_keys_by_pane() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "?");
        let screen = lines(&draw(&mut app, 200, 90));
        // The panes behind the popup have titles too, so look from the popup's own.
        let top = screen
            .iter()
            .position(|line| line.contains("Keys"))
            .unwrap();
        let row = |text: &str| {
            screen
                .iter()
                .skip(top)
                .position(|line| line.contains(text))
                .unwrap_or_else(|| panic!("no {text:?}"))
        };
        let groups = [
            "Everywhere",
            "Pull requests",
            "Checks",
            "Comments",
            "Description",
        ];
        let rows: Vec<usize> = groups.iter().map(|title| row(title)).collect();
        assert!(rows.windows(2).all(|pair| pair[0] < pair[1]), "{rows:?}");
        // Each group lists its own keys under its title.
        assert!(row("Review the highlighted PR") > row("Pull requests"));
        assert!(row("Open the highlighted check's page") > row("Checks"));
        assert!(row("Expand or collapse the highlighted comment") > row("Comments"));
        assert!(row("Scroll the description's tables sideways") > row("Description"));
    }

    /// A PR whose failing check has a page and whose passing one has none.
    fn app_with_check_pages() -> App {
        app_with(vec![pr_with(
            1,
            "Bump alpha",
            "OPEN",
            "",
            "MERGEABLE",
            serde_json::json!([
                {"__typename": "CheckRun", "name": "b", "workflowName": "CI",
                 "status": "COMPLETED", "conclusion": "FAILURE",
                 "detailsUrl": "https://example.test/job"},
                {"__typename": "CheckRun", "name": "c", "workflowName": "CI",
                 "status": "COMPLETED", "conclusion": "SUCCESS"},
            ]),
        )])
    }

    fn tab(app: &mut App, times: usize) {
        for _ in 0..times {
            app.update(Msg::Key(KeyCode::Tab.into()));
        }
    }

    fn status_bar(app: &mut App, width: u16) -> String {
        let screen = lines(&draw(app, width, 30));
        screen.last().unwrap().trim_end().to_string()
    }

    #[test]
    fn the_status_bar_shows_the_highlighted_checks_url_and_says_when_it_has_none() {
        let mut app = app_with_check_pages();
        // The table has no page to show.
        assert!(!status_bar(&mut app, 160).contains("https://"));

        tab(&mut app, 1);
        let bar = status_bar(&mut app, 160);
        assert!(
            bar.contains("Enter, o: Open | y: Copy | ?: Help | /: Search | q: Quit | https://example.test/job"),
            "{bar}"
        );

        // The passing check has no page, so there is no `o` hint either.
        press(&mut app, "j");
        let bar = status_bar(&mut app, 160);
        assert!(bar.ends_with("q: Quit | No page"), "{bar}");
        assert!(
            !bar.contains("o: Open") && !bar.contains("y: Copy"),
            "{bar}"
        );
    }

    #[test]
    fn the_status_bar_shows_the_highlighted_comments_url() {
        let mut app = app_with_check_pages();
        let details = serde_json::from_value(serde_json::json!({
            "headRefName": "branch",
            "body": "",
            "comments": [
                {"createdAt": "2026-01-02T03:00:00Z", "body": "a",
                 "url": "https://example.test/pull/1#issuecomment-1"},
                {"createdAt": "2026-01-02T03:00:00Z", "body": "b"},
            ],
            "additions": 1,
            "deletions": 1,
        }))
        .unwrap();
        app.details_loaded(1, Ok(details));
        tab(&mut app, 2);
        let bar = status_bar(&mut app, 160);
        assert!(
            bar.contains("Enter, Space: Expand | o: Open | y: Copy | ?: Help | /: Search | q: Quit | https://example.test/pull/1#issuecomment-1"),
            "{bar}"
        );
        // `o` still opens the PR for a comment with no page of its own.
        app.drawn.comment_layout = CommentLayout {
            items: vec![(0, 1), (1, 1)],
            viewport: 5,
        };
        press(&mut app, "j");
        let bar = status_bar(&mut app, 160);
        assert!(
            bar.contains(
                "Enter, Space: Expand | ?: Help | /: Search | q: Quit | No page, o opens the PR"
            ),
            "{bar}"
        );
        assert!(!bar.contains("o: Open"), "{bar}");
    }

    #[test]
    fn the_status_bar_cuts_a_long_url_then_drops_hints_but_keeps_o() {
        let long = "https://github.com/example/prism/actions/runs/123456789/job/987654321";
        let mut app = app_with(vec![pr_with(
            1,
            "Bump alpha",
            "OPEN",
            "",
            "MERGEABLE",
            serde_json::json!([
                {"__typename": "CheckRun", "name": "b", "workflowName": "CI",
                 "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": long},
            ]),
        )]);
        tab(&mut app, 1);
        // Wide enough for all of it.
        assert!(status_bar(&mut app, 200).contains(long));
        // Room for the hints, so only the URL is cut, in the middle.
        let bar = status_bar(&mut app, 110);
        assert!(
            bar.contains("https://github.com/") && bar.contains("…"),
            "{bar}"
        );
        assert!(
            bar.contains("o: Open | y: Copy | ?: Help | /: Search | q: Quit | https://"),
            "{bar}"
        );
        // Narrower still: the other hints go, `o` stays.
        let bar = status_bar(&mut app, 50);
        assert!(bar.contains("…") && bar.contains("Enter, o: Open"), "{bar}");
        assert!(!bar.contains("?: Help"), "{bar}");
        assert!(bar.chars().count() <= 50, "{bar}");
    }

    #[test]
    fn the_status_bar_says_what_was_copied_until_the_next_key() {
        let mut app = app_with_check_pages();
        tab(&mut app, 1);
        press(&mut app, "y");
        let bar = status_bar(&mut app, 160);
        assert!(bar.starts_with("Copied the URL | Enter, o: Open"), "{bar}");
        press(&mut app, "k");
        assert!(!status_bar(&mut app, 160).contains("Copied"));
    }

    /// What a failed `gh pr view` gives: a one-line message, then more.
    fn gh_failure() -> color_eyre::Report {
        crate::gh::GhError {
            command: "gh pr view 1 --json headRefName,comments".to_string(),
            output: "GraphQL: Could not resolve to a PullRequest with the number of 1.\nmore detail that is not for the pane".to_string(),
        }
        .into()
    }

    #[test]
    fn a_failed_details_fetch_shows_gh_s_first_line_in_the_comments_and_description() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.details_loaded(1, Err(gh_failure()));
        let screen = lines(&draw(&mut app, 200, 40)).join("\n");
        // Once in each of the Comments, Approvers and Description panes.
        let message = "Failed to load details: GraphQL: Could not resolve to a PullRequest";
        assert_eq!(screen.matches(message).count(), 3, "{screen}");
        assert!(!screen.contains("gh pr view"), "{screen}");
        assert!(!screen.contains("more detail"), "{screen}");
    }

    #[test]
    fn a_failed_diff_shows_gh_s_first_line_and_wraps() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        app.diff_loaded(1, Err(gh_failure()));
        // Narrow, so the message needs more than one row of the diff pane.
        let screen = lines(&draw(&mut app, 80, 40));
        let diff = app.drawn.areas.diff;
        let pane: Vec<String> = screen[usize::from(diff.y)..usize::from(diff.y + diff.height)]
            .iter()
            .map(|line| line.chars().skip(usize::from(diff.x)).collect())
            .collect();
        let pane = pane.join("\n");
        assert!(pane.contains("Failed to load diff: GraphQL"), "{pane}");
        // The end of the first line is there, on a later row, and nothing after it.
        assert!(pane.contains("number of 1."), "{pane}");
        assert!(
            !pane.contains("more detail") && !pane.contains("gh pr view"),
            "{pane}"
        );
    }

    #[test]
    fn the_status_bar_says_when_the_refresh_is_paused() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        assert!(!status_bar(&mut app, 160).contains("Refresh paused"));
        press(&mut app, "p");
        assert!(status_bar(&mut app, 160).starts_with("Refresh paused | "));
        press(&mut app, "p");
        app.update(Msg::FocusLost);
        assert!(status_bar(&mut app, 160).contains("Refresh paused"));
        app.update(Msg::FocusGained);
        assert!(!status_bar(&mut app, 160).contains("Refresh paused"));
    }

    #[test]
    fn cuts_text_in_the_middle() {
        assert_eq!(middle_cut("abcdefghij", 5), "ab…ij");
        assert_eq!(middle_cut("abcdefghij", 4), "ab…j");
        assert_eq!(middle_cut("abc", 5), "abc");
        assert_eq!(middle_cut("abc", 3), "abc");
    }
}
