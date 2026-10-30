use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table, TableState, Wrap};

use crate::app::{App, Batch, Confirm, Details, Diff, Mode, Pane, Summary};
use crate::gh::PrState;
use crate::model;

/// The most rows a list fetch error takes above the status bar.
const MAX_ERROR_ROWS: u16 = 5;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn view(app: &mut App, frame: &mut Frame) {
    let error_rows = app
        .error
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
    let [top_area, checks_area] =
        Layout::vertical([Constraint::Fill(2), Constraint::Fill(1)]).areas(left_area);
    let [search_area, table_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(top_area);

    frame.render_widget(Line::from(app.title.as_str()).bold().reversed(), title_area);
    search(app, frame, search_area);

    let table_block = pane_block("Pull requests", app.focus == Pane::Table);
    let table_inner = table_block.inner(table_area);
    frame.render_widget(table_block, table_area);
    let table_area = table_inner;
    if app.focus == Pane::Table {
        // One row of the table is its header.
        app.page = table_area.height.saturating_sub(2).max(1);
    }

    match app.mode {
        Mode::Loading => frame.render_widget(Line::from("Loading…"), table_area),
        Mode::List if app.rows.is_empty() => {
            let message = if app.error.is_some() {
                "No PRs loaded."
            } else {
                "No matching PRs found."
            };
            frame.render_widget(Line::from(message), table_area)
        }
        // Popups are drawn over the table.
        Mode::List | Mode::Confirm(_) | Mode::Running(_) | Mode::Results(_) => {
            let rows = app.rows.iter().map(|row| {
                let mark = if app.selected.contains(&row.pr.number) {
                    "[x]"
                } else {
                    "[ ]"
                };
                // The checks of a closed or merged PR no longer matter.
                let checks = match row.pr.state {
                    PrState::Open => row.checks.state.label(),
                    PrState::Closed => "· closed",
                    PrState::Merged => "· merged",
                };
                let cells = Row::new([
                    mark.to_string(),
                    format!("#{}", row.pr.number),
                    checks.to_string(),
                    model::review_label(&row.pr.review_decision).to_string(),
                    row.pr.title.clone(),
                ]);
                if row.pr.state == PrState::Open {
                    cells
                } else {
                    cells.dim()
                }
            });
            let widths = [
                Constraint::Length(3),
                Constraint::Length(7),
                Constraint::Length(12),
                Constraint::Length(11),
                Constraint::Fill(1),
            ];
            let mut table = Table::new(rows, widths)
                .header(Row::new(["", "#", "Checks", "Review", "Title"]).bold())
                .row_highlight_style(Style::new().reversed());
            // The rows are about to be replaced.
            if app.searching {
                table = table.dim();
            }
            frame.render_stateful_widget(table, table_area, &mut app.table);
        }
    }
    let [description_area, diff_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Fill(2)]).areas(detail_area);
    checks(app, frame, checks_area);
    description(app, frame, description_area);
    diff(app, frame, diff_area);

    if let Some(error) = app.error.as_deref() {
        let error = Paragraph::new(error).red().wrap(Wrap { trim: false });
        frame.render_widget(error, error_area);
    }
    let keys = match &app.mode {
        Mode::Confirm(confirm) => {
            confirm_dialog(confirm, app.dry_run, frame, body_area);
            "←/→ Tab: Choose | Enter: Confirm | Esc: Cancel"
        }
        Mode::Running(batch) => {
            running_popup(batch, app.dry_run, spinner(app), frame, body_area);
            "Ctrl-C: Quit"
        }
        Mode::Results(summary) => {
            results_popup(summary, app.dry_run, frame, body_area);
            "Enter/Esc: Close"
        }
        _ if app.editing => "Enter: Search | Esc: Cancel",
        _ => {
            "/: Search | Tab: Pane | ↑/↓ j/k PgUp/PgDn: Move | Space: Select | Enter: Review | q: Quit"
        }
    };
    let mut parts = Vec::new();
    if app.searching {
        parts.push(format!("{} Searching", spinner(app)));
    }
    if app.refreshing() {
        parts.push(format!("{} Refreshing", spinner(app)));
    }
    if !app.selected.is_empty() {
        parts.push(format!("{} selected", app.selected.len()));
    }
    parts.push(keys.to_string());
    let status = parts.join(" | ");
    frame.render_widget(
        Line::from(status).style(Style::new().reversed()),
        status_area,
    );
}

fn confirm_dialog(confirm: &Confirm, dry_run: bool, frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = confirm
        .prs
        .iter()
        .map(|pr| {
            Line::from(format!(
                "#{:<6} {:<12} {}",
                pr.number,
                pr.checks.label(),
                pr.title
            ))
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

fn running_popup(batch: &Batch, dry_run: bool, spinner: &str, frame: &mut Frame, area: Rect) {
    let (finished, total) = batch.progress();
    let progress = format!("{spinner} {finished}/{total} done");
    let step = if batch.is_merging() {
        "Merging"
    } else {
        "Approving"
    };
    let title = format!("{step} {}", count(total, "PR"));
    let lines = vec![Line::from(progress)];
    popup(frame, area, title, dry_run, lines, false, Vec::new());
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
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    frame.render_widget(content, content_area);
    frame.render_widget(Paragraph::new(footer), footer_area);
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

fn search(app: &App, frame: &mut Frame, area: Rect) {
    let width = area.width.saturating_sub(2);
    let scroll = app.query.visual_scroll(usize::from(width));
    let mut title = Line::from("Search");
    if app.searching {
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
    let block = pane_block("Checks", app.focus == Pane::Checks);
    let inner = block.inner(area);
    if app.focus == Pane::Checks {
        app.page = inner.height.saturating_sub(2).max(1);
    }
    frame.render_widget(block, area);
    let Some(row) = app.highlighted() else {
        return;
    };
    if row.checks.results.is_empty() {
        frame.render_widget(Line::from("No checks."), inner);
        return;
    }
    let rows: Vec<Row> = row
        .checks
        .worst_first()
        .into_iter()
        .map(|result| Row::new([result.state.label(), result.key.as_str()]))
        .collect();
    // One row of the table is its header.
    let scroll = app
        .checks_scroll
        .min(max_scroll(rows.len(), inner.height.saturating_sub(1)));
    let table = Table::new(rows, [Constraint::Length(10), Constraint::Fill(1)])
        .header(Row::new(["Status", "Check"]).bold());
    let mut state = TableState::default().with_offset(usize::from(scroll));
    frame.render_stateful_widget(table, inner, &mut state);
    app.checks_scroll = scroll;
}

fn diff(app: &mut App, frame: &mut Frame, area: Rect) {
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
        None | Some(Diff::Loading) => Text::from("Loading…"),
        Some(Diff::Loaded(diff)) => {
            scroll = app
                .diff_scroll
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
        Some(Diff::Failed(error)) => Text::from(format!("Failed to load diff: {error}")).red(),
    };
    frame.render_widget(Paragraph::new(text).block(block), area);
    app.diff_scroll = scroll;
}

fn description(app: &mut App, frame: &mut Frame, area: Rect) {
    let block = pane_block("Description", app.focus == Pane::Description);
    let inner = block.inner(area);
    if app.focus == Pane::Description {
        app.page = inner.height.saturating_sub(1).max(1);
    }
    let text = match app.highlighted() {
        None => Text::default(),
        Some(row) => match app.details.get(&row.pr.number) {
            None | Some(Details::Loading) => Text::from("Loading…"),
            Some(Details::Loaded { body, .. }) => Text::from(body.as_str()),
            Some(Details::Failed(error)) => {
                Text::from(format!("Failed to load details: {error}")).red()
            }
        },
    };
    let pane = Paragraph::new(text).wrap(Wrap { trim: false });
    let scroll = app
        .description_scroll
        .min(max_scroll(pane.line_count(inner.width), inner.height));
    frame.render_widget(pane.scroll((scroll, 0)).block(block), area);
    app.description_scroll = scroll;
}
