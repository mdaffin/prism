use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::gh::Pr;
use crate::model::{self, Checks};

pub struct App {
    pub title: String,
    pub mode: Mode,
    pub rows: Vec<Row>,
    /// The highlighted row.
    pub table: TableState,
    /// Why the last fetch failed.
    pub error: Option<String>,
    pub should_quit: bool,
}

pub enum Mode {
    Loading,
    List,
}

pub struct Row {
    pub pr: Pr,
    pub checks: Checks,
}

pub enum Msg {
    Key(KeyEvent),
    PrsLoaded(Result<Vec<Pr>>),
}

impl App {
    pub fn new(title: String) -> Self {
        Self {
            title,
            mode: Mode::Loading,
            rows: Vec::new(),
            table: TableState::default(),
            error: None,
            should_quit: false,
        }
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::PrsLoaded(Err(e)) => {
                self.error = Some(e.to_string());
                self.mode = Mode::List;
            }
            Msg::PrsLoaded(Ok(prs)) => {
                let mut rows: Vec<Row> = prs
                    .into_iter()
                    .map(|pr| {
                        let checks = model::classify_checks(&pr.mergeable, &pr.status_check_rollup);
                        Row { pr, checks }
                    })
                    .collect();
                // Passing first.
                rows.sort_by_key(|row| row.checks.state.rank());
                self.table.select((!rows.is_empty()).then_some(0));
                self.rows = rows;
                self.error = None;
                self.mode = Mode::List;
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if ctrl_c {
            self.should_quit = true;
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_highlight(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_highlight(-1),
            _ => {}
        }
    }

    fn move_highlight(&mut self, by: isize) {
        if let Some(current) = self.table.selected() {
            let last = self.rows.len().saturating_sub(1);
            self.table
                .select(Some(current.saturating_add_signed(by).min(last)));
        }
    }
}

/// `Open PRs (<repo> - <author> - <search>)`, leaving out empty parts, plus ` [DRY RUN]`.
pub fn title(repo: Option<&str>, author: Option<&str>, search: &str, dry_run: bool) -> String {
    let parts: Vec<&str> = [repo.unwrap_or(""), author.unwrap_or(""), search.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    let mut title = String::from("Open PRs");
    if !parts.is_empty() {
        title.push_str(&format!(" ({})", parts.join(" - ")));
    }
    if dry_run {
        title.push_str(" [DRY RUN]");
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_leaves_out_empty_parts() {
        assert_eq!(title(None, None, "", false), "Open PRs");
        assert_eq!(
            title(Some("o/r"), Some("app/renovate"), "label:deps", false),
            "Open PRs (o/r - app/renovate - label:deps)"
        );
        assert_eq!(
            title(None, Some("me"), " ", true),
            "Open PRs (me) [DRY RUN]"
        );
    }
}
