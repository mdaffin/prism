use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use regex::Regex;

use crate::app::{
    self, App, ChecksLayout, CommentLayout, ConfirmOpen, Details, Fetch, Mode, Pane, Typing, to_u16,
};
use crate::batch::{Batch, Confirm, ItemStatus, Summary};
use crate::gh::{Comment, PrState};
use crate::keys;
use crate::model::{self, CheckState};

/// The most rows a list fetch error takes above the status bar.
const MAX_ERROR_ROWS: u16 = 5;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The rows of the search and filter boxes: a line of text and the border around it.
const INPUT_BOX_ROWS: u16 = 3;

/// The most rows the comments pane takes.
const MAX_COMMENTS_ROWS: u16 = 10;

/// The rows of the Pull request box: the border, the title and the summary line.
const PULL_REQUEST_ROWS: u16 = 4;

/// The widest a popup gets, however wide the terminal is.
const MAX_POPUP_WIDTH: u16 = 90;

/// The widths of the table's columns other than the title. Each fits its longest value, such as
/// `#12345`, `· no checks` and `✓ approved`, and a space.
const NUMBER_WIDTH: u16 = 7;
const CHECKS_WIDTH: u16 = 12;
const REVIEW_WIDTH: u16 = 11;

/// The width of the checks pane's status column, which fits `… pending` and a space.
const CHECK_STATUS_WIDTH: u16 = 10;

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
    let [left_area, detail_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(body_area);
    let [top_area, checks_area, comments_area] = Layout::vertical([
        Constraint::Fill(2),
        Constraint::Fill(1),
        Constraint::Max(MAX_COMMENTS_ROWS),
    ])
    .areas(left_area);
    // The filter box is only there while it is being edited or has text in it.
    let filter_rows = if app.typing == Typing::Filter || !app.filter.value().is_empty() {
        INPUT_BOX_ROWS
    } else {
        0
    };
    let [search_area, filter_area, table_area] = Layout::vertical([
        Constraint::Length(INPUT_BOX_ROWS),
        Constraint::Length(filter_rows),
        Constraint::Fill(1),
    ])
    .areas(top_area);

    frame.render_widget(Line::from(app.title()).bold().reversed(), title_area);
    search(app, frame, search_area);
    if filter_rows > 0 {
        filter(app, frame, filter_area);
    }

    pr_table(app, frame, table_area);
    let [pull_request_area, description_area, diff_area] = Layout::vertical([
        Constraint::Length(PULL_REQUEST_ROWS),
        Constraint::Fill(1),
        Constraint::Fill(2),
    ])
    .areas(detail_area);
    checks(app, frame, checks_area);
    comments(app, frame, comments_area);
    pull_request(app, frame, pull_request_area);
    description(app, frame, description_area);
    diff(app, frame, diff_area);

    if let Some(error) = error {
        let error = Paragraph::new(error).red().wrap(Wrap { trim: false });
        frame.render_widget(error, error_area);
    }
    draw_dialog(app, frame, body_area);
    let status = status_line(app, &hints(app));
    frame.render_widget(
        Line::from(status).style(Style::new().reversed()),
        status_area,
    );
}

/// The popup over the table that the mode asks for, if any.
fn draw_dialog(app: &App, frame: &mut Frame, area: Rect) {
    match &app.mode {
        Mode::Confirm(confirm) => confirm_dialog(confirm, app.dry_run, frame, area),
        Mode::ConfirmOpen(confirm) => open_dialog(confirm, frame, area),
        Mode::Help => help_popup(frame, area),
        Mode::Running(batch) => running_popup(batch, app.dry_run, spinner(app), frame, area),
        Mode::Results(summary) => results_popup(summary, app.dry_run, frame, area),
        Mode::Loading | Mode::List => {}
    }
}

/// The keys that do something now, for the end of the status line.
fn hints(app: &App) -> String {
    let hints = match &app.mode {
        Mode::Confirm(_) | Mode::ConfirmOpen(_) => keys::CHOOSE_HINTS,
        Mode::Help => keys::HELP_HINTS,
        Mode::Running(_) => keys::RUNNING_HINTS,
        Mode::Results(_) => keys::RESULTS_HINTS,
        Mode::Loading | Mode::List => match app.typing {
            Typing::Search => keys::SEARCH_HINTS,
            Typing::Filter => keys::FILTER_HINTS,
            Typing::Nothing => return keys::bar(),
        },
    };
    hints.to_string()
}

/// The status bar: what is going on, then the `hints`.
fn status_line(app: &App, hints: &str) -> String {
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
    parts.push(hints.to_string());
    parts.join(" | ")
}

/// The width of the table's first column, which holds `[ ]` or `[x]`.
const CHECKBOX_WIDTH: u16 = 3;

/// The table of PRs in a bordered pane, or a message in its place while there are none.
fn pr_table(app: &mut App, frame: &mut Frame, area: Rect) {
    let table_block = pane_block("Pull requests", app.focus == Pane::Table);
    app.drawn.areas.table = area;
    app.drawn.areas.table_rows = Rect::default();
    app.drawn.areas.table_checkboxes = Rect::default();
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
        | Mode::Help
        | Mode::Running(_)
        | Mode::Results(_) => {
            // Borrowing the fields rather than calling `visible_rows` leaves `app.table` free.
            let rows = app.visible.iter().map(|&i| &app.rows[i]).map(|row| {
                let selected = app.selected.contains(&row.pr.number);
                pr_row(row, selected, app.columns.as_ref())
            });
            // The groups of the column regex share the space the title had.
            let titles = match &app.columns {
                Some(regex) => model::column_names(regex),
                None => vec!["Title".to_string()],
            };
            let widths = [
                Constraint::Length(CHECKBOX_WIDTH),
                Constraint::Length(NUMBER_WIDTH),
                Constraint::Length(CHECKS_WIDTH),
                Constraint::Length(REVIEW_WIDTH),
            ]
            .into_iter()
            .chain(titles.iter().map(|_| Constraint::Fill(1)))
            .collect::<Vec<_>>();
            let header = ["", "#", "Checks", "Review"]
                .map(str::to_string)
                .into_iter()
                .chain(titles);
            let mut table = Table::new(rows, widths)
                .header(Row::new(header).bold())
                .row_highlight_style(Style::new().reversed());
            // The rows are about to be replaced.
            if app.list.searching {
                table = table.dim();
            }
            frame.render_stateful_widget(table, table_area, &mut app.table);
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
        }
    }
}

/// A PR's row of the table. A closed or merged PR is dimmed.
fn pr_row<'a>(row: &'a app::Row, selected: bool, columns: Option<&Regex>) -> Row<'a> {
    let mark = if selected { "[x]" } else { "[ ]" };
    let checks = Cell::new(row.checks_label()).style(checks_style(row));
    let cells = Row::new(
        [
            Cell::new(mark),
            Cell::new(format!("#{}", row.pr.number)),
            checks,
            Cell::new(model::review_label(row.pr.review_decision)),
        ]
        .into_iter()
        .chain(row.title_cells(columns).into_iter().map(Cell::new)),
    );
    if row.pr.state == PrState::Open {
        cells
    } else {
        cells.dim()
    }
}

fn confirm_dialog(confirm: &Confirm, dry_run: bool, frame: &mut Frame, area: Rect) {
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
    footer.push(button_row(labels, confirm.choice));

    let title = format!("Review {}", count(confirm.prs.len(), "PR"));
    // One PR per line: long titles are cut off rather than wrapped.
    let dialog = Popup {
        title,
        dry_run,
        lines,
        wrap: false,
        footer,
    };
    popup(frame, area, dialog);
}

fn open_dialog(confirm: &ConfirmOpen, frame: &mut Frame, area: Rect) {
    let lines = vec![Line::from(pr_list(&confirm.numbers))];
    let highlighted = if confirm.open { 0 } else { 1 };
    let footer = vec![Line::default(), button_row(["Open", "Cancel"], highlighted)];
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
    };
    popup(frame, area, dialog);
}

fn help_popup(frame: &mut Frame, area: Rect) {
    let lines = keys::KEYS
        .iter()
        .map(|help| Line::from(format!("{:<16} {}", help.keys, help.action)))
        .collect();
    let footer = vec![Line::default(), Line::from("[ OK ]".reversed()).centered()];
    let help = Popup {
        title: "Keys".to_string(),
        dry_run: false,
        lines,
        wrap: false,
        footer,
    };
    popup(frame, area, help);
}

fn running_popup(batch: &Batch, dry_run: bool, spinner: &str, frame: &mut Frame, area: Rect) {
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
    };
    popup(frame, area, running);
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

fn results_popup(summary: &Summary, dry_run: bool, frame: &mut Frame, area: Rect) {
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
    let footer = vec![Line::default(), Line::from("[ OK ]".reversed()).centered()];
    // Errors can be long, so they wrap.
    let results = Popup {
        title: "Results".to_string(),
        dry_run,
        lines,
        wrap: true,
        footer,
    };
    popup(frame, area, results);
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
}

/// Draws a bordered popup in the middle of `area`. The footer lines are pinned to the bottom so
/// they stay visible even when `lines` doesn't fit. Wrapped lines are sized by estimate, so with
/// `wrap` the popup may cut off the end of `lines`.
fn popup(frame: &mut Frame, area: Rect, popup: Popup) {
    let Popup {
        mut title,
        dry_run,
        lines,
        wrap,
        footer,
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
    frame.buffer_mut().set_style(full, Modifier::DIM);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(content, content_area);
    frame.render_widget(Paragraph::new(footer), footer_area);
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
fn button_row<'a>(labels: impl IntoIterator<Item = &'a str>, highlighted: usize) -> Line<'static> {
    let mut buttons = Line::default();
    for (i, label) in labels.into_iter().enumerate() {
        let label = format!("[ {label} ]");
        buttons.push_span(if i == highlighted {
            label.reversed()
        } else {
            label.into()
        });
        buttons.push_span("  ");
    }
    buttons.centered()
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
    let mut block = Block::bordered().title("Filter");
    if app.typing == Typing::Filter {
        block = block.cyan();
    }
    let input = Paragraph::new(app.filter.value())
        .scroll((0, scroll as u16))
        .block(block);
    frame.render_widget(input, area);
    if app.typing == Typing::Filter {
        let x = app.filter.visual_cursor().saturating_sub(scroll) as u16;
        frame.set_cursor_position(Position::new(area.x + 1 + x, area.y + 1));
    }
}

fn search(app: &App, frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(2);
    let scroll = app.query.visual_scroll(usize::from(width));
    let mut title = Line::from("Search");
    if app.list.searching {
        title.push_span(format!(" · {} searching", spinner(app)).yellow().bold());
    }
    let mut block = Block::bordered().title(title);
    if app.typing == Typing::Search {
        block = block.cyan();
    }
    let input = Paragraph::new(app.query.value())
        .scroll((0, scroll as u16))
        .block(block);
    frame.render_widget(input, area);
    if app.typing == Typing::Search {
        let x = app.query.visual_cursor().saturating_sub(scroll) as u16;
        frame.set_cursor_position(Position::new(area.x + 1 + x, area.y + 1));
    }
}

/// A bordered pane, with a cyan border and title while it has focus.
fn pane_block(title: &str, focused: bool) -> Block<'_> {
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
fn max_scroll(content_rows: usize, area_rows: u16) -> u16 {
    let max = content_rows.saturating_sub(usize::from(area_rows));
    to_u16(max)
}

fn checks(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.checks = area;
    app.drawn.checks_layout = ChecksLayout::default();
    let block = pane_block("Checks", app.focus == Pane::Checks);
    let inner = block.inner(area);
    set_page(app, Pane::Checks, inner.height, 2);
    frame.render_widget(block, area);
    let Some(row) = app.highlighted() else {
        return;
    };
    // A conflict outranks the checks, so it gets a row of its own at the top.
    let conflict = (row.checks.state == CheckState::Conflict)
        .then(|| styled_check_row(CheckState::Conflict, "Merge conflict"));
    let first = usize::from(conflict.is_some());
    let results = row.checks.worst_first();
    let selected = app.scroll.check.min(results.len().saturating_sub(1));
    let rows: Vec<Row> = conflict
        .into_iter()
        .chain(results.iter().enumerate().map(|(i, result)| {
            let row = styled_check_row(result.state, &result.key);
            if i == selected { row.reversed() } else { row }
        }))
        .collect();
    let count = results.len();
    if rows.is_empty() {
        frame.render_widget(Line::from("No checks."), inner);
        return;
    }
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
    app.drawn.checks_layout = ChecksLayout {
        count,
        first,
        viewport: usize::from(inner.height.saturating_sub(1)),
    };
    app.scroll.check = selected;
    app.scroll.checks = scroll;
}

/// A row of the checks pane, with its status coloured.
fn styled_check_row<'a>(state: CheckState, name: &'a str) -> Row<'a> {
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
    let block = pane_block("Comments", app.focus == Pane::Comments);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (comments, bodies) = match app.highlighted().map(|row| app.details.get(&row.pr.number)) {
        None => return,
        Some(None | Some(Fetch::Loading)) => {
            frame.render_widget(Line::from("Loading…"), inner);
            return;
        }
        Some(Some(Fetch::Failed(error))) => {
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
        .map(|i| model::widest_table(&bodies[i]))
        .max()
        .unwrap_or(0);
    let x = usize::from(app.scroll.comment_x).min(widest.saturating_sub(width));

    let (document, items) = comment_document(
        comments,
        bodies,
        &app.scroll.comments_open,
        selected,
        inner.width,
        x,
    );
    let total = items.last().map_or(0, |&(start, height)| start + height);
    let pane = Paragraph::new(Text::from(document)).wrap(Wrap { trim: false });
    let scroll = app.scroll.comments.min(max_scroll(total, inner.height));
    frame.render_widget(pane.scroll((scroll, 0)), inner);

    set_page(app, Pane::Comments, inner.height, 1);
    app.drawn.comment_layout = CommentLayout {
        items,
        viewport: usize::from(inner.height),
    };
    app.scroll.comment = selected;
    app.scroll.comment_x = to_u16(x);
    app.scroll.comments = scroll;
}

/// The comments as one document: a header per comment, and the body under it when it is open.
/// Also each comment's first line and height, counted as wrapped to `width`, as that layout is
/// what `update` steps through.
fn comment_document(
    comments: &[Comment],
    bodies: &[Text<'static>],
    open: &HashSet<usize>,
    selected: usize,
    width: u16,
    x: usize,
) -> (Vec<Line<'static>>, Vec<(usize, usize)>) {
    let now = jiff::Timestamp::now();
    let mut document = Vec::new();
    let mut items = Vec::with_capacity(comments.len());
    let mut total = 0;
    for (i, comment) in comments.iter().enumerate() {
        let is_open = open.contains(&i);
        let author = comment
            .author
            .as_ref()
            .map_or("ghost", |a| a.login.as_str());
        let marker = if is_open { '▼' } else { '▶' };
        let mut header = Line::from(format!(
            "{marker} @{author} · {}",
            model::ago(&comment.created_at, now)
        ));
        if i == selected {
            header = header.reversed();
        }
        let mut lines = vec![header];
        if is_open {
            lines.extend(model::crop_tables(&bodies[i], usize::from(width), x).lines);
            lines.push(Line::default());
        }
        let height = Paragraph::new(Text::from(lines.clone()))
            .wrap(Wrap { trim: false })
            .line_count(width);
        items.push((total, height));
        total += height;
        document.extend(lines);
    }
    (document, items)
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
    let text = match diff {
        None if app.highlighted().is_none() => Text::default(),
        // A PR waiting for a free fetch slot has no entry yet.
        None | Some(Fetch::Loading) => Text::from("Loading…"),
        Some(Fetch::Loaded(diff)) => {
            scroll = app
                .scroll
                .diff
                .min(max_scroll(diff.lines.len(), inner.height));
            // Only the rows that fit are copied, as a diff can be very long.
            Text::from(
                diff.lines
                    .iter()
                    .skip(usize::from(scroll))
                    .take(usize::from(inner.height))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        }
        Some(Fetch::Failed(error)) => Text::from(format!("Failed to load diff: {error}")).red(),
    };
    frame.render_widget(Paragraph::new(text).block(block), area);
    app.scroll.diff = scroll;
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
        None | Some(Fetch::Loading) => summary.push(Span::raw("  Loading…")),
        Some(Fetch::Failed(_)) => {}
    }
    let title = Line::from(format!("#{} {}", row.pr.number, row.pr.title)).bold();
    frame.render_widget(Paragraph::new(vec![title, Line::from(summary)]), inner);
}

fn description(app: &mut App, frame: &mut Frame, area: Rect) {
    app.drawn.areas.description = area;
    // The block's inner area doesn't depend on its title.
    let inner = pane_block("", false).inner(area);
    set_page(app, Pane::Description, inner.height, 1);
    let width = usize::from(inner.width);
    let mut widest = 0;
    let mut x = 0;
    let text = match app.highlighted() {
        None => Text::default(),
        Some(row) => match app.details.get(&row.pr.number) {
            None | Some(Fetch::Loading) => Text::from("Loading…"),
            Some(Fetch::Loaded(Details { body, .. })) => {
                widest = model::widest_table(body);
                x = usize::from(app.scroll.description_x).min(widest.saturating_sub(width));
                model::crop_tables(body, width, x)
            }
            Some(Fetch::Failed(error)) => {
                Text::from(format!("Failed to load details: {error}")).red()
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
    let pane = Paragraph::new(text).wrap(Wrap { trim: false });
    let scroll = app
        .scroll
        .description
        .min(max_scroll(pane.line_count(inner.width), inner.height));
    frame.render_widget(pane.scroll((scroll, 0)).block(block), area);
    app.scroll.description = scroll;
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
    use crate::testing::{app_with, pr_with, press};

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
    fn draws_the_table() {
        let mut app = app_with(vec![
            pr(7, "Bump example-lib", "OPEN", "APPROVED"),
            pr(9, "Old change", "MERGED", ""),
        ]);
        let buffer = draw(&mut app, 160, 30);
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
        let buffer = draw(&mut app, 160, 30);
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
        let buffer = draw(&mut app, 160, 40);
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
        let buffer = draw(&mut app, 160, 40);
        assert!(find(&buffer, "Merge conflict").is_some());
        assert!(find(&buffer, "No checks.").is_none());
    }

    #[test]
    fn colours_the_checks_in_the_confirm_dialog() {
        let mut app = one_of_each();
        // Select everything, then open the dialog.
        press(&mut app, " j j j j ");
        app.update(Msg::Key(KeyCode::Enter.into()));
        let buffer = draw(&mut app, 160, 40);
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

    #[test]
    fn draws_the_filter_box_and_the_filtered_rows() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
            pr(3, "Update gamma", "OPEN", ""),
        ]);
        // No box until there is a filter.
        let screen = lines(&draw(&mut app, 160, 30));
        assert!(!screen.iter().any(|line| line.contains("Filter")));

        press(&mut app, " jjfgamma");
        let screen = lines(&draw(&mut app, 160, 30));
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
        let screen = lines(&draw(&mut app, 160, 30));

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

    #[test]
    fn the_filter_matches_the_columns_shown() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha to 1.2", "OPEN", ""),
            pr(2, "Bump beta to 3.4", "OPEN", ""),
        ]);
        app.columns =
            Some(model::parse_column_regex(r"Bump (?<container>.+?) to (?<version>\S+)$").unwrap());

        press(&mut app, "f3.4");
        let screen = lines(&draw(&mut app, 160, 30));
        assert!(screen.iter().any(|line| line.contains("#2")));
        assert!(!screen.iter().any(|line| line.contains("#1")));

        // "Bump" is in the titles but not in any column.
        for _ in 0..3 {
            app.update(Msg::Key(KeyCode::Backspace.into()));
        }
        press(&mut app, "Bump");
        let screen = lines(&draw(&mut app, 160, 30));
        assert!(screen.iter().any(|line| line.contains("No PRs match")));
    }

    #[test]
    fn shows_each_pr_of_a_running_batch_and_what_cancelling_did() {
        let mut app = app_with((1..=6).map(|n| pr(n, "Bump alpha", "OPEN", "")).collect());
        press(&mut app, " j j j j j ");
        app.update(Msg::Key(KeyCode::Enter.into()));
        app.update(Msg::Key(KeyCode::Enter.into()));
        // Four approvals run at once; two wait.
        assert_eq!(app.approvals_to_run().len(), 4);
        let epoch = app.batch_epoch();
        app.update(Msg::Approved(epoch, 1, Ok(())));
        let screen = lines(&draw(&mut app, 160, 30));
        // The popup draws over the table, so a status shares its line with a table row.
        let with = |text: &str| screen.iter().filter(|line| line.contains(text)).count();
        assert_eq!(with("✓ done"), 1);
        assert_eq!(with("… working"), 3);
        assert_eq!(with("· pending"), 2);
        assert!(screen.iter().any(|line| line.contains("q: Cancel")));

        press(&mut app, "q");
        let screen = lines(&draw(&mut app, 160, 30));
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
        let screen = lines(&draw(&mut app, 160, 30));
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
        app.update(Msg::DetailsLoaded(7, Ok(details)));
        let buffer = draw(&mut app, 160, 30);
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
        let plain = draw(&mut app, 160, 30);
        let table = find(&plain, "is:pr").unwrap();
        assert!(!plain[table].modifier.contains(Modifier::DIM));

        press(&mut app, "?");
        let buffer = draw(&mut app, 160, 30);
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
        draw(&mut app, 160, 40);
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
        app.update(Msg::DetailsLoaded(1, Ok(details)));
        app.update(Msg::DiffLoaded(1, Ok(vec!["+added"; 200].join("\n"))));
        draw(&mut app, 160, 40);
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
        let buffer = draw(&mut app, 160, 40);
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
        draw(&mut app, 160, 40);
        assert_eq!(app.scroll.check, 2);
        app.focus = Pane::Table;
        press(&mut app, "j");
        draw(&mut app, 160, 40);
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
        app.update(Msg::DetailsLoaded(1, Ok(details)));
        let screen = lines(&draw(&mut app, 160, 40));
        assert!(screen.iter().any(|line| line.contains("scroll tables")));
        assert!(!screen.iter().any(|line| line.contains("END")));

        app.focus = Pane::Description;
        for _ in 0..20 {
            press(&mut app, "l");
        }
        let screen = lines(&draw(&mut app, 160, 40));
        assert!(screen.iter().any(|line| line.contains("END")));
        // The prose stays where it was, and the scroll stops at the table's end.
        assert!(screen.iter().any(|line| line.contains("Some prose")));
        let at_end = app.scroll.description_x;
        press(&mut app, "l");
        draw(&mut app, 160, 40);
        assert_eq!(app.scroll.description_x, at_end);

        // And it scrolls back.
        press(&mut app, "h");
        assert!(app.scroll.description_x < at_end);
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
        app.update(Msg::DetailsLoaded(1, Ok(details)));
        app.focus = Pane::Comments;
        // Collapsed, there are only the headers.
        let screen = lines(&draw(&mut app, 160, 40));
        assert!(screen.iter().any(|line| line.contains("▶ @someone")));
        assert!(!screen.iter().any(|line| line.contains("Some bold prose")));

        press(&mut app, "j ");
        let screen = lines(&draw(&mut app, 160, 40));
        assert!(screen.iter().any(|line| line.contains("▼ @someone")));
        assert!(screen.iter().any(|line| line.contains("Some bold prose")));
        assert!(!screen.iter().any(|line| line.contains("END")));

        for _ in 0..20 {
            press(&mut app, "l");
        }
        let screen = lines(&draw(&mut app, 160, 40));
        assert!(screen.iter().any(|line| line.contains("END")));

        // Collapsing hides it again.
        press(&mut app, " ");
        let screen = lines(&draw(&mut app, 160, 40));
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
        draw(&mut app, 160, 40);
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
        draw(&mut app, 160, 40);
        // Scroll to the end, where the pane's scroll has been clamped, so the second comment's
        // header is some way down the pane.
        let second = app.drawn.comment_layout.items[1].0;
        app.scroll.comments = u16::MAX;
        draw(&mut app, 160, 40);
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
        draw(&mut app, 160, 40);
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
        draw(&mut app, 160, 40);
        let rows = app.drawn.areas.table_rows;
        click(&mut app, Position::new(rows.x + 5, rows.y + 2));
        assert_eq!(app.table.selected(), Some(2));
        assert_eq!(app.highlighted().unwrap().pr.number, 6);

        // Highlight the last row so the table scrolls, then click the first row drawn.
        for _ in 0..30 {
            press(&mut app, "j");
        }
        draw(&mut app, 160, 40);
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
        draw(&mut app, 160, 30);
        let rows = app.drawn.areas.table_rows;
        // The checkbox of the second row, while the first is highlighted.
        click(&mut app, Position::new(rows.x + 1, rows.y + 1));
        assert_eq!(app.selected, HashSet::from([2]));
        assert_eq!(app.table.selected(), Some(1));
        let buffer = draw(&mut app, 160, 30);
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
        draw(&mut app, 160, 30);
        let rows = app.drawn.areas.table_rows;
        click(&mut app, Position::new(rows.x + 5, rows.y + 1));
        assert_eq!(app.table.selected(), Some(1));
        click(&mut app, Position::new(rows.x + 5, rows.y - 1));
        click(&mut app, Position::new(rows.x + 5, rows.y + 5));
        assert_eq!(app.table.selected(), Some(1));
    }

    #[test]
    fn the_mouse_is_ignored_in_a_dialog_or_a_box() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        draw(&mut app, 160, 40);
        let diff = Position::new(app.drawn.areas.diff.x + 2, app.drawn.areas.diff.y + 1);
        press(&mut app, "?");
        click(&mut app, diff);
        assert!(app.focus == Pane::Table);
        press(&mut app, "q");
        press(&mut app, "f");
        click(&mut app, diff);
        assert!(app.focus == Pane::Table);
    }

    #[test]
    fn says_when_the_filter_hides_every_pr() {
        let mut app = app_with(vec![pr(1, "Bump alpha", "OPEN", "")]);
        press(&mut app, "fzzz");
        let screen = lines(&draw(&mut app, 160, 30));
        assert!(
            screen
                .iter()
                .any(|line| line.contains("No PRs match the filter."))
        );
    }
}
