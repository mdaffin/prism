use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Paragraph, Row, Table, Wrap};

use crate::app::{App, Details, Mode};
use crate::model;

/// The most rows a list fetch error takes above the status bar.
const MAX_ERROR_ROWS: u16 = 5;

pub fn view(app: &mut App, frame: &mut Frame) {
    let error = app
        .error
        .as_ref()
        .map(|error| format!("Search failed: {error}"));
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
    let [search_area, table_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(left_area);

    frame.render_widget(Line::from(app.title.as_str()).bold().reversed(), title_area);
    search(app, frame, search_area);

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
        Mode::List => {
            let rows = app.rows.iter().map(|row| {
                let mark = if app.selected.contains(&row.pr.number) {
                    "[x]"
                } else {
                    "[ ]"
                };
                Row::new([
                    mark.to_string(),
                    format!("#{}", row.pr.number),
                    row.checks.state.label().to_string(),
                    model::review_label(&row.pr.review_decision).to_string(),
                    row.pr.title.clone(),
                ])
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
    description(app, frame, detail_area);

    if let Some(error) = error {
        let error = Paragraph::new(error).red().wrap(Wrap { trim: false });
        frame.render_widget(error, error_area);
    }
    let keys = if app.editing {
        "Enter: Search | Esc: Cancel"
    } else {
        "/: Search | ↑/↓ j/k: Move | Space: Select | q: Quit"
    };
    let mut parts = Vec::new();
    if app.searching {
        parts.push("Searching…".to_string());
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
        title.push_span(" · searching…".yellow().bold());
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

fn description(app: &App, frame: &mut Frame, area: Rect) {
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
    let pane = Paragraph::new(text)
        .block(Block::bordered().title("Description"))
        .wrap(Wrap { trim: false });
    frame.render_widget(pane, area);
}
