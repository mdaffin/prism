use std::collections::HashMap;

use color_eyre::eyre::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::gh::{Pr, PrDetails};
use crate::model::{self, Checks};

/// The most detail fetches to run at once.
const MAX_DETAIL_FETCHES: usize = 4;

pub struct App {
    pub title: String,
    pub mode: Mode,
    /// The search box. While it isn't being edited it holds the last submitted query.
    pub query: Input,
    /// The query to restore when an edit is cancelled.
    submitted_query: String,
    pub editing: bool,
    /// A query submitted but not yet fetched.
    query_to_fetch: Option<String>,
    /// The id of the latest list fetch. Results from older fetches are dropped.
    fetch_id: u64,
    /// Whether the latest list fetch is still running.
    pub searching: bool,
    pub rows: Vec<Row>,
    /// The highlighted row.
    pub table: TableState,
    /// Why the last list fetch failed.
    pub error: Option<String>,
    /// Details by PR number. An entry is added when its fetch starts, so a PR is fetched once.
    pub details: HashMap<u64, Details>,
    pub should_quit: bool,
}

pub enum Mode {
    /// No list has been fetched yet.
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
    /// The result of the list fetch with this id.
    PrsLoaded(u64, Result<Vec<Pr>>),
    DetailsLoaded(u64, Result<PrDetails>),
}

impl App {
    pub fn new(title: String, query: String) -> Self {
        Self {
            title,
            mode: Mode::Loading,
            query: Input::new(query.clone()),
            submitted_query: query.clone(),
            editing: false,
            query_to_fetch: Some(query),
            fetch_id: 0,
            searching: false,
            rows: Vec::new(),
            table: TableState::default(),
            error: None,
            details: HashMap::new(),
            should_quit: false,
        }
    }

    /// A submitted query and the id of its fetch, which the caller must start.
    pub fn list_to_fetch(&mut self) -> Option<(u64, String)> {
        let query = self.query_to_fetch.take()?;
        self.fetch_id += 1;
        self.searching = true;
        Some((self.fetch_id, query))
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
            Msg::PrsLoaded(id, _) if id != self.fetch_id => {}
            Msg::PrsLoaded(_, Err(e)) => {
                // The previous results stay.
                self.error = Some(e.to_string());
                self.searching = false;
                self.mode = Mode::List;
            }
            Msg::PrsLoaded(_, Ok(prs)) => {
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
                // Give failed details another chance with the new results.
                self.details
                    .retain(|_, details| !matches!(details, Details::Failed(_)));
                self.error = None;
                self.searching = false;
                self.mode = Mode::List;
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if ctrl_c {
            self.should_quit = true;
        } else if self.editing {
            self.on_search_key(key);
        } else {
            self.on_list_key(key);
        }
    }

    /// While editing, every key other than Enter and Esc goes to the search box.
    fn on_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.editing = false;
                self.submitted_query = self.query.value().to_string();
                self.query_to_fetch = Some(self.submitted_query.clone());
                self.error = None;
            }
            KeyCode::Esc => {
                self.editing = false;
                self.query = Input::new(self.submitted_query.clone());
            }
            _ => {
                self.query.handle_event(&Event::Key(key));
            }
        }
    }

    fn on_list_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('/') => self.editing = true,
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

/// `PRs (<repo>)`, plus ` [DRY RUN]`. The query isn't included: it can change, and the search
/// box shows it.
pub fn title(repo: Option<&str>, dry_run: bool) -> String {
    let mut title = String::from("PRs");
    if let Some(repo) = repo {
        title.push_str(&format!(" ({repo})"));
    }
    if dry_run {
        title.push_str(" [DRY RUN]");
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prs(numbers: impl IntoIterator<Item = u64>) -> Vec<Pr> {
        numbers
            .into_iter()
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
            .collect()
    }

    fn app_with_rows(count: u64) -> App {
        let mut app = App::new(String::new(), "is:pr".to_string());
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=count))));
        app
    }

    fn press(app: &mut App, keys: &str) {
        for c in keys.chars() {
            app.update(Msg::Key(KeyEvent::from(KeyCode::Char(c))));
        }
    }

    fn numbers(app: &App) -> Vec<u64> {
        app.rows.iter().map(|row| row.pr.number).collect()
    }

    #[test]
    fn fetches_the_initial_query_once() {
        let mut app = App::new(String::new(), "is:pr is:open".to_string());
        assert_eq!(app.list_to_fetch(), Some((1, "is:pr is:open".to_string())));
        assert_eq!(app.list_to_fetch(), None);
    }

    #[test]
    fn search_box_takes_every_key_while_editing() {
        let mut app = app_with_rows(3);
        press(&mut app, "/");
        assert!(app.editing);
        press(&mut app, " q/jk");
        assert!(!app.should_quit);
        assert_eq!(app.query.value(), "is:pr q/jk");
        assert_eq!(app.table.selected(), Some(0));
    }

    #[test]
    fn esc_restores_the_submitted_query() {
        let mut app = app_with_rows(3);
        press(&mut app, "/ x");
        app.update(Msg::Key(KeyEvent::from(KeyCode::Esc)));
        assert!(!app.editing);
        assert_eq!(app.query.value(), "is:pr");
        assert_eq!(app.list_to_fetch(), None);
    }

    #[test]
    fn enter_fetches_the_new_query_and_resets_the_highlight() {
        let mut app = app_with_rows(3);
        app.table.select(Some(2));
        press(&mut app, "/ is:closed");
        app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
        assert!(!app.editing);
        let (id, query) = app.list_to_fetch().unwrap();
        assert_eq!(query, "is:pr is:closed");
        assert!(app.searching);

        app.update(Msg::PrsLoaded(id, Ok(prs([7, 8]))));
        assert!(!app.searching);
        assert_eq!(numbers(&app), [7, 8]);
        assert_eq!(app.table.selected(), Some(0));
    }

    #[test]
    fn results_from_an_older_fetch_are_dropped() {
        let mut app = app_with_rows(3);
        press(&mut app, "/");
        app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
        let (old, _) = app.list_to_fetch().unwrap();
        press(&mut app, "/");
        app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
        let (new, _) = app.list_to_fetch().unwrap();

        app.update(Msg::PrsLoaded(new, Ok(prs([5]))));
        app.update(Msg::PrsLoaded(old, Ok(prs([6]))));
        assert_eq!(numbers(&app), [5]);
    }

    #[test]
    fn a_failed_search_keeps_the_old_results() {
        let mut app = app_with_rows(3);
        press(&mut app, "/");
        app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(
            id,
            Err(color_eyre::eyre::eyre!("bad query")),
        ));
        assert_eq!(app.error.as_deref(), Some("bad query"));
        assert!(!app.searching);
        assert_eq!(numbers(&app), [1, 2, 3]);
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
    fn title_shows_repo_and_dry_run() {
        assert_eq!(title(None, false), "PRs");
        assert_eq!(title(Some("o/r"), false), "PRs (o/r)");
        assert_eq!(title(Some("o/r"), true), "PRs (o/r) [DRY RUN]");
    }
}
