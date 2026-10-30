use std::collections::HashMap;

use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

use crate::gh::{Pr, PrDetails};
use crate::model::{self, Checks};

/// The most detail fetches to run at once.
const MAX_DETAIL_FETCHES: usize = 4;

pub struct App {
    pub title: String,
    pub mode: Mode,
    pub rows: Vec<Row>,
    /// The highlighted row.
    pub table: TableState,
    /// Why the last fetch failed.
    pub error: Option<String>,
    /// Details by PR number. An entry is added when its fetch starts, so a PR is fetched once.
    pub details: HashMap<u64, Details>,
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

pub enum Details {
    Loading,
    Loaded {
        // Read by the detail panes (M2).
        #[allow(dead_code)]
        details: PrDetails,
        /// The body with HTML comments and extra blank lines removed.
        body: String,
    },
    /// Kept rather than removed, so a failing PR isn't fetched again on every update.
    Failed(String),
}

pub enum Msg {
    Key(KeyEvent),
    PrsLoaded(Result<Vec<Pr>>),
    DetailsLoaded(u64, Result<PrDetails>),
}

impl App {
    pub fn new(title: String) -> Self {
        Self {
            title,
            mode: Mode::Loading,
            rows: Vec::new(),
            table: TableState::default(),
            error: None,
            details: HashMap::new(),
            should_quit: false,
        }
    }

    /// PRs whose details haven't been requested yet, as many as there are free fetch slots.
    /// The highlighted PR comes first, then the rows below it, then those above, so a fetch
    /// started as a slot frees up goes to the rows about to be viewed. Marks them as loading,
    /// so the caller must start the fetches.
    pub fn details_to_fetch(&mut self) -> Vec<u64> {
        let Some(start) = self.table.selected() else {
            return Vec::new();
        };
        let in_flight = self
            .details
            .values()
            .filter(|details| matches!(details, Details::Loading))
            .count();
        let numbers: Vec<u64> = (start..self.rows.len())
            .chain(0..start)
            .map(|i| self.rows[i].pr.number)
            .filter(|number| !self.details.contains_key(number))
            .take(MAX_DETAIL_FETCHES.saturating_sub(in_flight))
            .collect();
        for &number in &numbers {
            self.details.insert(number, Details::Loading);
        }
        numbers
    }

    pub fn highlighted(&self) -> Option<&Row> {
        self.table.selected().and_then(|i| self.rows.get(i))
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::DetailsLoaded(number, Err(e)) => {
                self.details.insert(number, Details::Failed(e.to_string()));
            }
            Msg::DetailsLoaded(number, Ok(details)) => {
                let body = model::clean_body(details.body.as_deref());
                self.details
                    .insert(number, Details::Loaded { details, body });
            }
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

    fn app_with_rows(count: u64) -> App {
        let mut app = App::new(String::new());
        let prs: Vec<Pr> = (1..=count)
            .map(|number| {
                serde_json::from_value(serde_json::json!({
                    "number": number,
                    "title": "t",
                    "state": "OPEN",
                    "statusCheckRollup": [],
                    "mergeable": "MERGEABLE",
                    "reviewDecision": "",
                }))
                .unwrap()
            })
            .collect();
        app.update(Msg::PrsLoaded(Ok(prs)));
        app
    }

    fn finish(app: &mut App, number: u64) {
        app.update(Msg::DetailsLoaded(
            number,
            Err(color_eyre::eyre::eyre!("failed")),
        ));
    }

    #[test]
    fn prefetches_details_from_the_highlight_within_the_limit() {
        let mut app = app_with_rows(8);
        assert_eq!(app.details_to_fetch(), [1, 2, 3, 4]);
        // Every slot is busy.
        assert!(app.details_to_fetch().is_empty());

        // Freed slots go to the highlighted row and those below it, then wrap to the top.
        app.table.select(Some(5));
        finish(&mut app, 1);
        assert_eq!(app.details_to_fetch(), [6]);
        finish(&mut app, 2);
        finish(&mut app, 3);
        assert_eq!(app.details_to_fetch(), [7, 8]);
        finish(&mut app, 4);
        assert_eq!(app.details_to_fetch(), [5]);

        // Nothing is fetched twice, including failures.
        for number in 5..=8 {
            finish(&mut app, number);
        }
        assert!(app.details_to_fetch().is_empty());
    }

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
