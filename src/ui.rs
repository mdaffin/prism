use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};

use crate::app::{App, ConfirmOpen, Details, Fetch, Mode, Pane};
use crate::batch::{Batch, Confirm, ItemStatus, Summary};
use crate::gh::PrState;
use crate::keys;
use crate::model::{self, CheckState};

/// The most rows a list fetch error takes above the status bar.
const MAX_ERROR_ROWS: u16 = 5;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
        Constraint::Max(10),
    ])
    .areas(left_area);
    // The filter box is only there while it is being edited or has text in it.
    let filter_rows = if app.filtering || !app.filter.value().is_empty() {
        3
    } else {
        0
    };
    let [search_area, filter_area, table_area] = Layout::vertical([
        Constraint::Length(3),
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
        Constraint::Length(4),
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
    let hints = match &app.mode {
        Mode::Confirm(confirm) => {
            confirm_dialog(confirm, app.dry_run, frame, body_area);
            "←/→ Tab: Choose | Enter: Confirm | Esc: Cancel"
        }
        Mode::ConfirmOpen(confirm) => {
            open_dialog(confirm, frame, body_area);
            "←/→ Tab: Choose | Enter: Confirm | Esc: Cancel"
        }
        Mode::Help => {
            help_popup(frame, body_area);
            "Esc ? q: Close"
        }
        Mode::Running(batch) => {
            running_popup(batch, app.dry_run, spinner(app), frame, body_area);
            "q: Cancel | Ctrl-C: Quit"
        }
        Mode::Results(summary) => {
            results_popup(summary, app.dry_run, frame, body_area);
            "Enter/Esc: Close"
        }
        _ if app.editing => "Enter: Search | Esc: Cancel",
        _ if app.filtering => "Enter: Keep filter | Esc: Clear filter",
        _ => &keys::bar(),
    };
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
    let status = parts.join(" | ");
    frame.render_widget(
        Line::from(status).style(Style::new().reversed()),
        status_area,
    );
}

/// The table of PRs in a bordered pane, or a message in its place while there are none.
fn pr_table(app: &mut App, frame: &mut Frame, area: Rect) {
    let table_block = pane_block("Pull requests", app.focus == Pane::Table);
    app.areas.table = area;
    app.areas.table_rows = Rect::default();
    let table_area = table_block.inner(area);
    frame.render_widget(table_block, area);
    if app.focus == Pane::Table {
        // One row of the table is its header.
        app.page = table_area.height.saturating_sub(2).max(1);
    }

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
                let mark = if app.selected.contains(&row.pr.number) {
                    "[x]"
                } else {
                    "[ ]"
                };
                let checks = Cell::new(row.checks_label());
                // The checks of a closed or merged PR no longer matter, so they aren't coloured.
                let checks = if row.pr.state == PrState::Open {
                    checks.style(state_style(row.checks.state))
                } else {
                    checks
                };
                let cells = Row::new(
                    [
                        Cell::new(mark),
                        Cell::new(format!("#{}", row.pr.number)),
                        checks,
                        Cell::new(model::review_label(row.pr.review_decision)),
                    ]
                    .into_iter()
                    .chain(
                        row.title_cells(app.columns.as_ref())
                            .into_iter()
                            .map(Cell::new),
                    ),
                );
                if row.pr.state == PrState::Open {
                    cells
                } else {
                    cells.dim()
                }
            });
            // The groups of the column regex share the space the title had.
            let titles = match &app.columns {
                Some(regex) => model::column_names(regex),
                None => vec!["Title".to_string()],
            };
            let widths = [
                Constraint::Length(3),
                Constraint::Length(7),
                Constraint::Length(12),
                Constraint::Length(11),
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
            app.areas.table_rows = Rect {
                y: table_area.y + 1,
                height: table_area.height.saturating_sub(1),
                ..table_area
            };
            app.areas.table_offset = app.table.offset();
        }
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
        let numbers: Vec<String> = confirm.left_out.iter().map(|n| format!("#{n}")).collect();
        footer.push(Line::default());
        footer.push(
            Line::from(format!("Left out closed or merged: {}", numbers.join(", "))).yellow(),
        );
    }
    footer.push(Line::default());
    let mut buttons = Line::default();
    for (i, choice) in confirm.choices().iter().enumerate() {
        let label = format!("[ {} ]", choice.label());
        buttons.push_span(if i == confirm.choice {
            label.reversed()
        } else {
            label.into()
        });
        buttons.push_span("  ");
    }
    footer.push(buttons.centered());

    let title = format!("Review {}", count(confirm.prs.len(), "PR"));
    // One PR per line: long titles are cut off rather than wrapped.
    popup(frame, area, title, dry_run, lines, false, footer);
}

fn open_dialog(confirm: &ConfirmOpen, frame: &mut Frame, area: Rect) {
    let numbers: Vec<String> = confirm.numbers.iter().map(|n| format!("#{n}")).collect();
    let lines = vec![Line::from(numbers.join(", "))];
    let mut buttons = Line::default();
    for (label, highlighted) in [("Open", confirm.open), ("Cancel", !confirm.open)] {
        let label = format!("[ {label} ]");
        buttons.push_span(if highlighted {
            label.reversed()
        } else {
            label.into()
        });
        buttons.push_span("  ");
    }
    let footer = vec![Line::default(), buttons.centered()];
    let title = format!(
        "Open {} in the browser?",
        count(confirm.numbers.len(), "PR")
    );
    popup(frame, area, title, false, lines, true, footer);
}

fn help_popup(frame: &mut Frame, area: Rect) {
    let lines = keys::KEYS
        .iter()
        .map(|help| Line::from(format!("{:<16} {}", help.keys, help.action)))
        .collect();
    let footer = vec![Line::default(), Line::from("[ OK ]".reversed()).centered()];
    popup(frame, area, "Keys".to_string(), false, lines, false, footer);
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
    popup(frame, area, title, dry_run, lines, true, footer);
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
        let numbers: Vec<String> = summary
            .interrupted
            .iter()
            .map(|n| format!("#{n}"))
            .collect();
        lines.push(Line::default());
        lines.push(
            Line::from(format!(
                "Interrupted while running - check GitHub: {}",
                numbers.join(", ")
            ))
            .yellow(),
        );
    }
    if !summary.auto_merge.is_empty() {
        let numbers: Vec<String> = summary.auto_merge.iter().map(|n| format!("#{n}")).collect();
        lines.push(Line::default());
        lines.push(
            Line::from(format!(
                "Auto-merge queued (checks still pending): {}",
                numbers.join(", ")
            ))
            .yellow(),
        );
    }
    let footer = vec![Line::default(), Line::from("[ OK ]".reversed()).centered()];
    // Errors can be long, so they wrap.
    popup(
        frame,
        area,
        "Results".to_string(),
        dry_run,
        lines,
        true,
        footer,
    );
}

/// Draws a bordered popup in the middle of `area`. The `footer` lines, such as buttons, are
/// pinned to the bottom so they stay visible even when `lines` doesn't fit. Wrapped lines are
/// sized by estimate, so with `wrap` the popup may cut off the end of `lines`.
fn popup(
    frame: &mut Frame,
    area: Rect,
    mut title: String,
    dry_run: bool,
    lines: Vec<Line>,
    wrap: bool,
    footer: Vec<Line>,
) {
    if dry_run {
        title.push_str(" [DRY RUN]");
    }
    let full = area;
    let width = area.width.saturating_sub(4).min(90);
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
    if app.filtering {
        block = block.cyan();
    }
    let input = Paragraph::new(app.filter.value())
        .scroll((0, scroll as u16))
        .block(block);
    frame.render_widget(input, area);
    if app.filtering {
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
    if app.editing {
        block = block.cyan();
    }
    let input = Paragraph::new(app.query.value())
        .scroll((0, scroll as u16))
        .block(block);
    frame.render_widget(input, area);
    if app.editing {
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

/// The furthest a pane can scroll: until its last row is at the bottom.
fn max_scroll(content_rows: usize, area_rows: u16) -> u16 {
    let max = content_rows.saturating_sub(usize::from(area_rows));
    u16::try_from(max).unwrap_or(u16::MAX)
}

fn checks(app: &mut App, frame: &mut Frame, area: Rect) {
    app.areas.checks = area;
    let block = pane_block("Checks", app.focus == Pane::Checks);
    let inner = block.inner(area);
    if app.focus == Pane::Checks {
        app.page = inner.height.saturating_sub(2).max(1);
    }
    frame.render_widget(block, area);
    let Some(row) = app.highlighted() else {
        return;
    };
    // A conflict outranks the checks, so it gets a row of its own at the top.
    let conflict = (row.checks.state == CheckState::Conflict)
        .then(|| styled_check_row(CheckState::Conflict, "Merge conflict"));
    let rows: Vec<Row> = conflict
        .into_iter()
        .chain(
            row.checks
                .worst_first()
                .into_iter()
                .map(|result| styled_check_row(result.state, &result.key)),
        )
        .collect();
    if rows.is_empty() {
        frame.render_widget(Line::from("No checks."), inner);
        return;
    }
    // One row of the table is its header.
    let scroll = app
        .scroll
        .checks
        .min(max_scroll(rows.len(), inner.height.saturating_sub(1)));
    let table = Table::new(rows, [Constraint::Length(10), Constraint::Fill(1)])
        .header(Row::new(["Status", "Check"]).bold());
    let mut state = TableState::default().with_offset(usize::from(scroll));
    frame.render_stateful_widget(table, inner, &mut state);
    app.scroll.checks = scroll;
}

/// A row of the checks pane, with its status coloured.
fn styled_check_row<'a>(state: CheckState, name: &'a str) -> Row<'a> {
    Row::new([
        Cell::new(state.label()).style(state_style(state)),
        Cell::new(name),
    ])
}

/// The comments of the highlighted PR: a list of who and when beside a preview of the selected
/// one.
fn comments(app: &mut App, frame: &mut Frame, area: Rect) {
    app.areas.comments = area;
    app.areas.comment_preview = Rect::default();
    let block = pane_block("Comments", app.focus == Pane::Comments);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let comments = match app.highlighted().map(|row| app.details.get(&row.pr.number)) {
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
        Some(Some(Fetch::Loaded(Details { details, .. }))) => &details.comments,
    };
    if comments.is_empty() {
        frame.render_widget(Line::from("No comments."), inner);
        return;
    }
    let [list_area, preview_area] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Fill(1)]).areas(inner);
    let selected = app.scroll.comment.min(comments.len() - 1);

    let now = jiff::Timestamp::now();
    let rows = comments.iter().map(|comment| {
        let author = comment
            .author
            .as_ref()
            .map_or("ghost", |a| a.login.as_str());
        Row::new([format!("@{author}"), model::ago(&comment.created_at, now)])
    });
    let list = Table::new(rows, [Constraint::Fill(1), Constraint::Length(14)])
        .row_highlight_style(Style::new().reversed());
    let mut state = TableState::default().with_selected(selected);
    frame.render_stateful_widget(list, list_area, &mut state);

    let preview_block = Block::new().borders(Borders::LEFT);
    app.areas.comment_preview = preview_area;
    let preview_inner = preview_block.inner(preview_area);
    let body = model::clean_body(comments[selected].body.as_deref());
    let preview = Paragraph::new(body).wrap(Wrap { trim: false });
    let scroll = app.scroll.comment_scroll.min(max_scroll(
        preview.line_count(preview_inner.width),
        preview_inner.height,
    ));
    frame.render_widget(
        preview.scroll((scroll, 0)).block(preview_block),
        preview_area,
    );
    if app.focus == Pane::Comments {
        app.page = preview_inner.height.saturating_sub(1).max(1);
    }
    app.scroll.comment = selected;
    app.scroll.comment_scroll = scroll;
}

fn diff(app: &mut App, frame: &mut Frame, area: Rect) {
    app.areas.diff = area;
    let block = pane_block("Diff", app.focus == Pane::Diff);
    let inner = block.inner(area);
    if app.focus == Pane::Diff {
        app.page = inner.height.saturating_sub(1).max(1);
    }
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
    let checks = Span::raw(row.checks_label());
    // The checks of a closed or merged PR no longer matter, so they aren't coloured.
    let checks = if row.pr.state == PrState::Open {
        checks.style(state_style(row.checks.state))
    } else {
        checks
    };
    let mut summary = vec![
        checks,
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
    app.areas.description = area;
    let block = pane_block("Description", app.focus == Pane::Description);
    let inner = block.inner(area);
    if app.focus == Pane::Description {
        app.page = inner.height.saturating_sub(1).max(1);
    }
    let text = match app.highlighted() {
        None => Text::default(),
        Some(row) => match app.details.get(&row.pr.number) {
            None | Some(Fetch::Loading) => Text::from("Loading…"),
            Some(Fetch::Loaded(Details { body, .. })) => Text::from(body.as_str()),
            Some(Fetch::Failed(error)) => {
                Text::from(format!("Failed to load details: {error}")).red()
            }
        },
    };
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
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;

    use super::*;
    use crate::app::Msg;
    use crate::gh::Pr;

    fn pr(number: u64, title: &str, state: &str, review: &str) -> Pr {
        serde_json::from_value(serde_json::json!({
            "number": number,
            "title": title,
            "state": state,
            "statusCheckRollup": [],
            "mergeable": "MERGEABLE",
            "reviewDecision": review,
            "updatedAt": "2026-01-01T00:00:00Z",
        }))
        .unwrap()
    }

    /// An app that has loaded these PRs, in the order given before the table sorts them.
    fn app_with(prs: Vec<Pr>) -> App {
        let mut app = App::new("PRs".to_string(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs)));
        app
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
        serde_json::from_value(serde_json::json!({
            "number": number,
            "title": format!("PR {number}"),
            "state": "OPEN",
            "statusCheckRollup": checks,
            "mergeable": mergeable,
            "reviewDecision": "",
            "updatedAt": "2026-01-01T00:00:00Z",
        }))
        .unwrap()
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

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            app.update(Msg::Key(KeyCode::Char(c).into()));
        }
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
            (Pane::Checks, app.areas.checks),
            (Pane::Comments, app.areas.comments),
            (Pane::Description, app.areas.description),
            (Pane::Diff, app.areas.diff),
            (Pane::Table, app.areas.table),
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
        list: Position,
        preview: Position,
    }

    #[test]
    fn the_wheel_scrolls_the_pane_under_the_pointer_without_moving_focus() {
        let mut app = app_with_long_panes();
        let inside = |area: Rect| Position::new(area.x + 2, area.y + 1);
        let areas = &app.areas;
        let at = Points {
            table: inside(areas.table),
            checks: inside(areas.checks),
            description: inside(areas.description),
            diff: inside(areas.diff),
            list: inside(areas.comments),
            preview: inside(areas.comment_preview),
        };

        wheel(&mut app, at.table, true);
        assert_eq!(app.table.selected(), Some(1));
        wheel(&mut app, at.table, false);
        assert_eq!(app.table.selected(), Some(0));

        wheel(&mut app, at.checks, true);
        assert_eq!(app.scroll.checks, 3);
        wheel(&mut app, at.description, true);
        assert_eq!(app.scroll.description, 3);
        wheel(&mut app, at.diff, true);
        wheel(&mut app, at.diff, true);
        assert_eq!(app.scroll.diff, 6);
        wheel(&mut app, at.diff, false);
        assert_eq!(app.scroll.diff, 3);

        // Over the comments list it picks a comment; over the preview it scrolls that.
        let list = at.list;
        assert!(!app.areas.comment_preview.contains(list));
        wheel(&mut app, list, true);
        assert_eq!(app.scroll.comment, 1);
        let preview = at.preview;
        wheel(&mut app, preview, true);
        assert_eq!(app.scroll.comment, 1);
        assert_eq!(app.scroll.comment_scroll, 3);

        assert!(app.focus == Pane::Table);
    }

    #[test]
    fn the_wheel_is_ignored_in_a_dialog() {
        let mut app = app_with_long_panes();
        let diff = Position::new(app.areas.diff.x + 2, app.areas.diff.y + 1);
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
        let rows = app.areas.table_rows;
        click(&mut app, Position::new(rows.x + 5, rows.y + 2));
        assert_eq!(app.table.selected(), Some(2));
        assert_eq!(app.highlighted().unwrap().pr.number, 6);

        // Highlight the last row so the table scrolls, then click the first row drawn.
        for _ in 0..30 {
            press(&mut app, "j");
        }
        draw(&mut app, 160, 40);
        let offset = app.areas.table_offset;
        assert!(offset > 0);
        let rows = app.areas.table_rows;
        click(&mut app, Position::new(rows.x + 5, rows.y));
        assert_eq!(app.table.selected(), Some(offset));
        assert_eq!(
            app.highlighted().unwrap().pr.number,
            2 * (offset as u64 + 1)
        );
    }

    #[test]
    fn a_click_on_the_header_or_below_the_rows_does_nothing() {
        let mut app = app_with(vec![
            pr(1, "Bump alpha", "OPEN", ""),
            pr(2, "Bump beta", "OPEN", ""),
        ]);
        draw(&mut app, 160, 30);
        let rows = app.areas.table_rows;
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
        let diff = Position::new(app.areas.diff.x + 2, app.areas.diff.y + 1);
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
