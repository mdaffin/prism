use std::collections::{HashMap, HashSet};

use color_eyre::eyre::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::gh::{Pr, PrDetails, PrState};
use crate::model::{self, CheckState, Checks};

/// The most detail fetches to run at once.
const MAX_DETAIL_FETCHES: usize = 4;

/// The most approvals to run at once.
const MAX_APPROVALS: usize = 4;

/// The most row refreshes to run at once.
const MAX_REFRESHES: usize = 4;

pub struct App {
    pub title: String,
    pub dry_run: bool,
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
    /// The numbers of the PRs marked with Space.
    pub selected: HashSet<u64>,
    /// Why the last search or row refreshes failed, shown above the status bar.
    pub error: Option<String>,
    /// Details by PR number. An entry is added when its fetch starts, so a PR is fetched once.
    pub details: HashMap<u64, Details>,
    /// Rows to fetch again after a batch, so they show their new state without replacing the
    /// list.
    to_refresh: Vec<u64>,
    /// The PRs whose rows are being refreshed.
    refreshing: HashSet<u64>,
    /// Advances on each tick while something is animating.
    pub spinner: usize,
    pub should_quit: bool,
}

pub enum Mode {
    /// No list has been fetched yet.
    Loading,
    List,
    /// Asking what to do with the selected PRs.
    Confirm(Confirm),
    /// Approving PRs.
    Running(Batch),
    /// Showing how a batch went.
    Results(Summary),
}

pub struct Batch {
    pub items: Vec<BatchItem>,
}

pub struct BatchItem {
    pub pr: ConfirmPr,
    pub status: ItemStatus,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ItemStatus {
    Pending,
    Working,
    Done,
    /// The error, on one line.
    Failed(String),
}

impl Batch {
    pub fn finished(&self) -> usize {
        self.items
            .iter()
            .filter(|item| matches!(item.status, ItemStatus::Done | ItemStatus::Failed(_)))
            .count()
    }
}

#[derive(Debug)]
pub struct Summary {
    pub approved: usize,
    pub total: usize,
    /// PR numbers and their errors, on one line each.
    pub failures: Vec<(u64, String)>,
}

/// The confirm dialog. It keeps its own copy of the PRs, so a list that loads while it is open
/// doesn't change what it lists.
pub struct Confirm {
    /// The open PRs to act on.
    pub prs: Vec<ConfirmPr>,
    /// Closed and merged PRs that were selected but are left out.
    pub left_out: Vec<u64>,
    /// The highlighted choice, an index into [`Confirm::choices`].
    pub choice: usize,
}

pub struct ConfirmPr {
    pub number: u64,
    pub checks: CheckState,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Approve,
    ApproveAndMerge,
    Cancel,
}

impl Choice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Approve => "Approve",
            Self::ApproveAndMerge => "Approve & merge",
            Self::Cancel => "Cancel",
        }
    }
}

impl Confirm {
    /// Only Cancel when there is nothing to act on.
    pub fn choices(&self) -> &'static [Choice] {
        if self.prs.is_empty() {
            &[Choice::Cancel]
        } else {
            &[Choice::Approve, Choice::ApproveAndMerge, Choice::Cancel]
        }
    }
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
    /// The result of approving this PR.
    Approved(u64, Result<()>),
    /// The result of fetching this PR again to refresh its row.
    PrRefreshed(u64, Result<Pr>),
    /// Time to advance the spinner.
    Tick,
}

impl App {
    pub fn new(title: String, query: String, dry_run: bool) -> Self {
        Self {
            title,
            dry_run,
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
            selected: HashSet::new(),
            details: HashMap::new(),
            to_refresh: Vec::new(),
            refreshing: HashSet::new(),
            spinner: 0,
            should_quit: false,
        }
    }

    /// Whether the spinner needs ticks.
    pub fn animating(&self) -> bool {
        self.searching || self.refreshing() || matches!(self.mode, Mode::Running(_))
    }

    /// Whether any rows are being refreshed or are waiting to be.
    pub fn refreshing(&self) -> bool {
        !self.refreshing.is_empty() || !self.to_refresh.is_empty()
    }

    /// Rows to refresh now, up to [`MAX_REFRESHES`] at once. The caller must start them.
    pub fn refreshes_to_run(&mut self) -> Vec<u64> {
        let free = MAX_REFRESHES.saturating_sub(self.refreshing.len());
        let numbers: Vec<u64> = self
            .to_refresh
            .drain(..free.min(self.to_refresh.len()))
            .collect();
        self.refreshing.extend(&numbers);
        numbers
    }

    /// Approvals that can start now, up to [`MAX_APPROVALS`] at once. Marks them as working,
    /// so the caller must start them.
    pub fn approvals_to_run(&mut self) -> Vec<u64> {
        let Mode::Running(batch) = &mut self.mode else {
            return Vec::new();
        };
        let working = batch
            .items
            .iter()
            .filter(|item| item.status == ItemStatus::Working)
            .count();
        batch
            .items
            .iter_mut()
            .filter(|item| item.status == ItemStatus::Pending)
            .take(MAX_APPROVALS.saturating_sub(working))
            .map(|item| {
                item.status = ItemStatus::Working;
                item.pr.number
            })
            .collect()
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
            Msg::Tick => self.spinner = self.spinner.wrapping_add(1),
            Msg::Approved(number, result) => self.approved(number, result),
            Msg::PrRefreshed(number, result) => {
                self.refreshing.remove(&number);
                match result {
                    Ok(pr) => {
                        if let Some(row) = self.rows.iter_mut().find(|row| row.pr.number == number)
                        {
                            row.checks =
                                model::classify_checks(&pr.mergeable, &pr.status_check_rollup);
                            row.pr = pr;
                        }
                    }
                    // The row keeps its old state, so say why it didn't change.
                    Err(e) => {
                        let message = format!("Refreshing #{number} failed: {e}");
                        self.error = Some(match self.error.take() {
                            Some(error) => format!("{error}\n{message}"),
                            None => message,
                        });
                    }
                }
            }
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
                self.error = Some(format!("Search failed: {e}"));
                self.searching = false;
                self.loaded();
            }
            Msg::PrsLoaded(_, Ok(prs)) => {
                let mut rows: Vec<Row> = prs
                    .into_iter()
                    .map(|pr| {
                        let checks = model::classify_checks(&pr.mergeable, &pr.status_check_rollup);
                        Row { pr, checks }
                    })
                    .collect();
                // Open PRs first, passing first; closed and merged PRs can't be acted on.
                rows.sort_by_key(|row| (row.pr.state != PrState::Open, row.checks.state.rank()));
                self.table.select((!rows.is_empty()).then_some(0));
                self.rows = rows;
                self.selected.clear();
                // Give failed details another chance with the new results.
                self.details
                    .retain(|_, details| !matches!(details, Details::Failed(_)));
                self.error = None;
                self.searching = false;
                self.loaded();
            }
        }
    }

    /// Records an approval's result. After the last one, shows the summary, clears the selection
    /// and refreshes the batch's rows so they show their new review states.
    fn approved(&mut self, number: u64, result: Result<()>) {
        let Mode::Running(batch) = &mut self.mode else {
            return;
        };
        if let Some(item) = batch.items.iter_mut().find(|item| item.pr.number == number) {
            item.status = match result {
                Ok(()) => ItemStatus::Done,
                Err(e) => ItemStatus::Failed(one_line(&e.to_string())),
            };
        }
        if batch.finished() < batch.items.len() {
            return;
        }
        let failures = batch
            .items
            .iter()
            .filter_map(|item| match &item.status {
                ItemStatus::Failed(e) => Some((item.pr.number, e.clone())),
                _ => None,
            })
            .collect();
        let total = batch.items.len();
        let approved = batch
            .items
            .iter()
            .filter(|item| item.status == ItemStatus::Done)
            .count();
        // Only the batch's rows are refreshed. Running the search again could reorder the list
        // or drop PRs the user still means to work through.
        self.to_refresh
            .extend(batch.items.iter().map(|item| item.pr.number));
        self.mode = Mode::Results(Summary {
            approved,
            total,
            failures,
        });
        self.selected.clear();
    }

    /// Leaves `Loading` once a list fetch finishes, without closing an open dialog.
    fn loaded(&mut self) {
        if matches!(self.mode, Mode::Loading) {
            self.mode = Mode::List;
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if ctrl_c {
            self.should_quit = true;
        } else if let Mode::Confirm(confirm) = &mut self.mode {
            if let Some(choice) = on_confirm_key(confirm, key) {
                self.choose(choice);
            }
        } else if matches!(self.mode, Mode::Running(_)) {
            // Cancelling a batch comes in M4 step 3.
        } else if matches!(self.mode, Mode::Results(_)) {
            if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
                self.mode = Mode::List;
            }
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
            KeyCode::Char(' ') => self.toggle_selected(),
            KeyCode::Enter => self.open_confirm(),
            _ => {}
        }
    }

    /// Opens the confirm dialog for the selected PRs, or the highlighted one if none are
    /// selected, in table order.
    fn open_confirm(&mut self) {
        let rows: Vec<&Row> = if self.selected.is_empty() {
            self.highlighted().into_iter().collect()
        } else {
            self.rows
                .iter()
                .filter(|row| self.selected.contains(&row.pr.number))
                .collect()
        };
        if rows.is_empty() {
            return;
        }
        let (open, left_out): (Vec<&Row>, Vec<&Row>) = rows
            .into_iter()
            .partition(|row| row.pr.state == PrState::Open);
        let prs = open
            .into_iter()
            .map(|row| ConfirmPr {
                number: row.pr.number,
                checks: row.checks.state,
                title: row.pr.title.clone(),
            })
            .collect();
        let left_out = left_out.iter().map(|row| row.pr.number).collect();
        self.mode = Mode::Confirm(Confirm {
            prs,
            left_out,
            choice: 0,
        });
    }

    fn choose(&mut self, choice: Choice) {
        let Mode::Confirm(confirm) = std::mem::replace(&mut self.mode, Mode::List) else {
            return;
        };
        match choice {
            Choice::Approve => {
                // Errors from earlier no longer apply.
                self.error = None;
                let items = confirm
                    .prs
                    .into_iter()
                    .map(|pr| BatchItem {
                        pr,
                        status: ItemStatus::Pending,
                    })
                    .collect();
                self.mode = Mode::Running(Batch { items });
            }
            // Merging comes in M1 step 14.
            Choice::ApproveAndMerge | Choice::Cancel => {}
        }
    }

    fn toggle_selected(&mut self) {
        if let Some(number) = self.highlighted().map(|row| row.pr.number)
            && !self.selected.remove(&number)
        {
            self.selected.insert(number);
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

/// Joins the lines of `text` with spaces, so it fits in one row.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Moves between the dialog's choices, and returns the choice made, if any.
fn on_confirm_key(confirm: &mut Confirm, key: KeyEvent) -> Option<Choice> {
    let last = confirm.choices().len() - 1;
    match key.code {
        KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
            confirm.choice = confirm.choice.saturating_sub(1);
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
            confirm.choice = (confirm.choice + 1).min(last);
        }
        KeyCode::Enter => return Some(confirm.choices()[confirm.choice]),
        KeyCode::Esc | KeyCode::Char('q') => return Some(Choice::Cancel),
        _ => {}
    }
    None
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

    fn pr(number: u64, state: &str) -> Pr {
        serde_json::from_value(serde_json::json!({
            "number": number,
            "title": "t",
            "state": state,
            "statusCheckRollup": [],
            "mergeable": "MERGEABLE",
            "reviewDecision": "",
        }))
        .unwrap()
    }

    fn prs(numbers: impl IntoIterator<Item = u64>) -> Vec<Pr> {
        numbers
            .into_iter()
            .map(|number| pr(number, "OPEN"))
            .collect()
    }

    fn app_with_rows(count: u64) -> App {
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
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
        let mut app = App::new(String::new(), "is:pr state:open".to_string(), false);
        assert_eq!(
            app.list_to_fetch(),
            Some((1, "is:pr state:open".to_string()))
        );
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
    fn space_toggles_the_highlighted_pr_until_new_results() {
        let mut app = app_with_rows(3);
        press(&mut app, " jj ");
        assert_eq!(app.selected, HashSet::from([1, 3]));
        press(&mut app, "k ");
        assert_eq!(app.selected, HashSet::from([1, 2, 3]));
        press(&mut app, " ");
        assert_eq!(app.selected, HashSet::from([1, 3]));

        // Not cleared until the new results arrive.
        press(&mut app, "/");
        app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
        let (id, _) = app.list_to_fetch().unwrap();
        assert_eq!(app.selected.len(), 2);
        app.update(Msg::PrsLoaded(id, Ok(prs([1, 2, 3]))));
        assert!(app.selected.is_empty());
    }

    fn key(app: &mut App, code: KeyCode) {
        app.update(Msg::Key(KeyEvent::from(code)));
    }

    fn confirm(app: &App) -> &Confirm {
        match &app.mode {
            Mode::Confirm(confirm) => confirm,
            _ => panic!("the confirm dialog isn't open"),
        }
    }

    fn confirm_numbers(app: &App) -> Vec<u64> {
        confirm(app).prs.iter().map(|pr| pr.number).collect()
    }

    #[test]
    fn enter_confirms_the_highlighted_pr_when_none_are_selected() {
        let mut app = app_with_rows(3);
        press(&mut app, "j");
        key(&mut app, KeyCode::Enter);
        assert_eq!(confirm_numbers(&app), [2]);
        assert_eq!(confirm(&app).choices().len(), 3);
    }

    #[test]
    fn enter_confirms_the_selected_prs_in_table_order() {
        let mut app = app_with_rows(3);
        press(&mut app, " jj ");
        key(&mut app, KeyCode::Enter);
        assert_eq!(confirm_numbers(&app), [1, 3]);
    }

    #[test]
    fn closed_and_merged_prs_sort_after_open_ones() {
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        let prs = vec![
            pr(1, "MERGED"),
            pr(2, "OPEN"),
            pr(3, "CLOSED"),
            pr(4, "OPEN"),
        ];
        app.update(Msg::PrsLoaded(id, Ok(prs)));
        assert_eq!(numbers(&app), [2, 4, 1, 3]);
    }

    #[test]
    fn closed_and_merged_prs_are_left_out() {
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        let prs = vec![pr(1, "OPEN"), pr(2, "CLOSED"), pr(3, "MERGED")];
        app.update(Msg::PrsLoaded(id, Ok(prs)));
        press(&mut app, " j j ");
        key(&mut app, KeyCode::Enter);
        assert_eq!(confirm_numbers(&app), [1]);
        assert_eq!(confirm(&app).left_out, [2, 3]);

        // With only closed PRs there is nothing to choose but Cancel.
        key(&mut app, KeyCode::Esc);
        press(&mut app, "k");
        app.selected.clear();
        key(&mut app, KeyCode::Enter);
        assert!(confirm(&app).prs.is_empty());
        assert_eq!(confirm(&app).choices(), [Choice::Cancel]);
    }

    #[test]
    fn the_dialog_moves_between_choices_and_closes() {
        let mut app = app_with_rows(3);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Right);
        assert_eq!(confirm(&app).choice, 1);
        for _ in 0..5 {
            key(&mut app, KeyCode::Tab);
        }
        assert_eq!(confirm(&app).choice, 2);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::List));

        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::List));
        assert!(!app.should_quit);
    }

    #[test]
    fn a_list_loading_under_the_dialog_keeps_it_open() {
        let mut app = app_with_rows(3);
        press(&mut app, "/");
        key(&mut app, KeyCode::Enter);
        let (id, _) = app.list_to_fetch().unwrap();
        key(&mut app, KeyCode::Enter);
        app.update(Msg::PrsLoaded(id, Ok(prs([7]))));
        assert_eq!(confirm_numbers(&app), [1]);
    }

    /// Selects the first `count` rows and chooses Approve.
    fn start_approving(app: &mut App, count: usize) {
        for _ in 0..count {
            press(app, " j");
        }
        key(app, KeyCode::Enter);
        key(app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Running(_)));
    }

    fn approve(app: &mut App, number: u64, result: Result<()>) {
        app.update(Msg::Approved(number, result));
    }

    #[test]
    fn approvals_run_at_most_four_at_once() {
        let mut app = app_with_rows(6);
        start_approving(&mut app, 6);
        assert_eq!(app.approvals_to_run(), [1, 2, 3, 4]);
        assert!(app.approvals_to_run().is_empty());
        approve(&mut app, 2, Ok(()));
        assert_eq!(app.approvals_to_run(), [5]);
        approve(&mut app, 1, Ok(()));
        approve(&mut app, 3, Ok(()));
        assert_eq!(app.approvals_to_run(), [6]);
        assert!(app.approvals_to_run().is_empty());
    }

    #[test]
    fn keys_are_ignored_while_approving() {
        let mut app = app_with_rows(2);
        start_approving(&mut app, 1);
        for code in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Enter] {
            key(&mut app, code);
        }
        assert!(matches!(app.mode, Mode::Running(_)));
        assert!(!app.should_quit);
    }

    #[test]
    fn the_last_approval_shows_results_and_refreshes_the_batch() {
        let mut app = app_with_rows(4);
        start_approving(&mut app, 3);
        app.approvals_to_run();
        approve(&mut app, 1, Ok(()));
        approve(&mut app, 3, Err(color_eyre::eyre::eyre!("no\npermission")));
        assert!(matches!(app.mode, Mode::Running(_)));
        assert!(app.refreshes_to_run().is_empty());

        approve(&mut app, 2, Ok(()));
        let Mode::Results(summary) = &app.mode else {
            panic!("the results aren't shown");
        };
        assert_eq!((summary.approved, summary.total), (2, 3));
        assert_eq!(summary.failures, [(3, "no permission".to_string())]);
        assert!(app.selected.is_empty());
        // Only the batch's rows are fetched again, not the whole search.
        assert_eq!(app.list_to_fetch(), None);
        assert_eq!(app.refreshes_to_run(), [1, 2, 3]);

        key(&mut app, KeyCode::Char('q'));
        assert!(matches!(app.mode, Mode::List));
        assert!(!app.should_quit);
    }

    #[test]
    fn refreshed_rows_update_in_place() {
        let mut app = app_with_rows(3);
        app.table.select(Some(1));
        app.to_refresh = vec![1, 2, 3];
        assert_eq!(app.refreshes_to_run(), [1, 2, 3]);
        assert!(app.refreshing());

        // A merged PR would sort last, but the rows keep their order.
        let mut merged = pr(1, "MERGED");
        merged.review_decision = "APPROVED".to_string();
        app.update(Msg::PrRefreshed(1, Ok(merged)));
        app.update(Msg::PrRefreshed(2, Err(color_eyre::eyre::eyre!("failed"))));
        app.update(Msg::PrRefreshed(9, Ok(pr(9, "OPEN"))));
        assert!(app.refreshing());
        app.update(Msg::PrRefreshed(
            3,
            Err(color_eyre::eyre::eyre!("timed out")),
        ));
        assert!(!app.refreshing());

        assert_eq!(numbers(&app), [1, 2, 3]);
        assert_eq!(app.rows[0].pr.state, PrState::Merged);
        assert_eq!(app.rows[0].pr.review_decision, "APPROVED");
        assert_eq!(app.table.selected(), Some(1));
        // Failed refreshes are reported, and the rows keep their old state.
        assert_eq!(
            app.error.as_deref(),
            Some("Refreshing #2 failed: failed\nRefreshing #3 failed: timed out")
        );
        assert_eq!(app.rows[1].pr.state, PrState::Open);
    }

    #[test]
    fn refreshes_run_at_most_four_at_once() {
        let mut app = app_with_rows(6);
        app.to_refresh = (1..=6).collect();
        assert_eq!(app.refreshes_to_run(), [1, 2, 3, 4]);
        assert!(app.refreshes_to_run().is_empty());
        app.update(Msg::PrRefreshed(1, Ok(pr(1, "OPEN"))));
        assert_eq!(app.refreshes_to_run(), [5]);
    }

    #[test]
    fn a_failed_search_keeps_the_old_results() {
        let mut app = app_with_rows(3);
        press(&mut app, " ");
        press(&mut app, "/");
        app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(
            id,
            Err(color_eyre::eyre::eyre!("bad query")),
        ));
        assert_eq!(app.error.as_deref(), Some("Search failed: bad query"));
        assert!(!app.searching);
        assert_eq!(numbers(&app), [1, 2, 3]);
        assert_eq!(app.selected, HashSet::from([1]));
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
