use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Row, Table, Wrap};

use crate::app::{App, Mode};
use crate::model;

pub fn view(app: &mut App, frame: &mut Frame) {
    let [title_area, body_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(Line::from(app.title.as_str()).bold().reversed(), title_area);

    if let Some(error) = &app.error {
        let message = Paragraph::new(format!("Failed to load PRs: {error}"))
            .red()
            .wrap(Wrap { trim: false });
        frame.render_widget(message, body_area);
    } else {
        match app.mode {
            Mode::Loading => frame.render_widget(Line::from("Loading…"), body_area),
            Mode::List if app.rows.is_empty() => {
                frame.render_widget(Line::from("No matching PRs found."), body_area)
            }
            Mode::List => {
                let rows = app.rows.iter().map(|row| {
                    Row::new([
                        format!("#{}", row.pr.number),
                        row.checks.state.label().to_string(),
                        model::review_label(&row.pr.review_decision).to_string(),
                        row.pr.title.clone(),
                    ])
                });
                let widths = [
                    Constraint::Length(7),
                    Constraint::Length(12),
                    Constraint::Length(11),
                    Constraint::Fill(1),
                ];
                let table = Table::new(rows, widths)
                    .header(Row::new(["#", "Checks", "Review", "Title"]).bold())
                    .row_highlight_style(Style::new().reversed());
                frame.render_stateful_widget(table, body_area, &mut app.table);
            }
        }
    }

    frame.render_widget(
        Line::from("↑/↓ j/k: Move | q: Quit").style(Style::new().reversed()),
        status_area,
    );
}
