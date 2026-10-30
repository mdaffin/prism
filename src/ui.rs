use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};

use crate::app::{App, Batch, Confirm, ConfirmOpen, Details, Fetch, Mode, Pane, Summary};
use crate::gh::PrState;
use crate::keys;
use crate::model::{self, CheckState};

/// The most rows a list fetch error takes above the status bar.
const MAX_ERROR_ROWS: u16 = 5;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn view(app: &mut App, frame: &mut Frame) {
    let error = app.error_text();
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

    frame.render_widget(Line::from(app.title.as_str()).bold().reversed(), title_area);
    search(app, frame, search_area);
    if filter_rows > 0 {
        filter(app, frame, filter_area);
    }

    pr_table(app, frame, table_area);
    let [description_area, diff_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Fill(2)]).areas(detail_area);
    checks(app, frame, checks_area);
    comments(app, frame, comments_area);
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
            "Ctrl-C: Quit"
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
    if app.searching {
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
            let message = if app.error_text().is_some() {
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
                let cells = Row::new([
                    Cell::new(mark),
                    Cell::new(format!("#{}", row.pr.number)),
                    checks,
                    Cell::new(model::review_label(row.pr.review_decision)),
                    Cell::new(row.pr.title.as_str()),
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
        .checks_scroll
        .min(max_scroll(rows.len(), inner.height.saturating_sub(1)));
    let table = Table::new(rows, [Constraint::Length(10), Constraint::Fill(1)])
        .header(Row::new(["Status", "Check"]).bold());
    let mut state = TableState::default().with_offset(usize::from(scroll));
    frame.render_stateful_widget(table, inner, &mut state);
    app.checks_scroll = scroll;
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
    let selected = app.comment.min(comments.len() - 1);

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
    let preview_inner = preview_block.inner(preview_area);
    let body = model::clean_body(comments[selected].body.as_deref());
    let preview = Paragraph::new(body).wrap(Wrap { trim: false });
    let scroll = app.comment_scroll.min(max_scroll(
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
    app.comment = selected;
    app.comment_scroll = scroll;
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
        None | Some(Fetch::Loading) => Text::from("Loading…"),
        Some(Fetch::Loaded(diff)) => {
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
        Some(Fetch::Failed(error)) => Text::from(format!("Failed to load diff: {error}")).red(),
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
            None | Some(Fetch::Loading) => Text::from("Loading…"),
            Some(Fetch::Loaded(Details { body, .. })) => Text::from(body.as_str()),
            Some(Fetch::Failed(error)) => {
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
        let open = screen.iter().find(|line| line.contains("#7")).unwrap();
        assert!(open.contains("[ ]"));
        assert!(open.contains("· no checks"));
        assert!(open.contains("✓ approved"));
        assert!(open.contains("Bump example-lib"));
        let merged = screen.iter().find(|line| line.contains("#9")).unwrap();
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
