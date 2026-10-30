use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode};
use crate::model;

pub fn view(app: &App, frame: &mut Frame) {
    let [title_area, body_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(Line::from(app.title.as_str()).bold().reversed(), title_area);

    // A plain list of rows until the PR table (M1 step 6).
    let body: Vec<Line> = match app.mode {
        Mode::Loading => vec![Line::from("Loading…")],
        Mode::List if app.rows.is_empty() => vec![Line::from("No matching PRs found.")],
        Mode::List => app
            .rows
            .iter()
            .map(|row| {
                let (ticket, title) = model::split_ticket(&row.pr.title);
                Line::from(format!(
                    "#{} {:?}  {}  {}  {}  {}",
                    row.pr.number,
                    row.pr.state,
                    row.checks.state.label(),
                    model::review_label(&row.pr.review_decision),
                    ticket,
                    title,
                ))
            })
            .collect(),
    };
    frame.render_widget(Paragraph::new(body), body_area);

    frame.render_widget(
        Line::from("q: Quit").style(Style::new().reversed()),
        status_area,
    );
}
