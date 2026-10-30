use std::collections::{HashMap, HashSet};

use ansi_to_tui::IntoText;
use color_eyre::eyre::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Text;
use ratatui::widgets::TableState;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::gh::{Pr, PrDetails, PrState};
use crate::model::{self, CheckState, Checks};

/// The most detail fetches to run at once.
const MAX_DETAIL_FETCHES: usize = 4;

/// The most diffs fetched at once.
const MAX_DIFF_FETCHES: usize = 4;

/// The most approvals to run at once.
const MAX_APPROVALS: usize = 4;

/// The most failed row refreshes to list above the status bar.
const MAX_REFRESH_ERRORS: usize = 3;

/// The most row refreshes to run at once.
const MAX_REFRESHES: usize = 4;

/// The most PRs Ctrl-O opens without asking first.
const MAX_OPEN_WITHOUT_ASKING: usize = 5;

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
    /// The pane that Up/Down and PgUp/PgDn act on.
    pub focus: Pane,
    /// Rows scrolled off the top of the description and diff panes. They go back to the top
    /// when the highlight moves. The view clamps them to the content it draws.
    pub checks_scroll: u16,
    pub description_scroll: u16,
    pub diff_scroll: u16,
    /// The selected comment of the highlighted PR. The view clamps it to the comments there are.
    pub comment: usize,
    /// Rows scrolled off the top of the selected comment's preview.
    pub comment_scroll: u16,
    /// The rows a PgUp/PgDn moves in the focused pane, set by the view as it draws.
    pub page: u16,
    /// The numbers of the PRs marked with Space.
    pub selected: HashSet<u64>,
    /// Why the last search failed, shown above the status bar.
    pub error: Option<String>,
    /// One line for each row refresh that failed, shown after `error`.
    refresh_errors: Vec<String>,
    /// One line for each PR that failed to open in the browser, shown after `refresh_errors`.
    open_errors: Vec<String>,
    /// PRs to open in the browser, in order.
    opens_to_run: Vec<u64>,
    /// Details by PR number. An entry is added when its fetch starts, so a PR is fetched once.
    pub details: HashMap<u64, Fetch<Details>>,
    /// Diffs by PR number. An entry is added when its fetch starts, so a PR is fetched once.
    pub diffs: HashMap<u64, Fetch<Diff>>,
    /// Rows to fetch again after a batch, so they show their new state without replacing the
    /// list.
    to_refresh: Vec<u64>,
    /// The PRs whose rows are being refreshed.
    refreshing: HashSet<u64>,
    /// Advances on each tick while something is animating.
    pub spinner: usize,
    pub should_quit: bool,
}

/// The panes that can have focus. Tab cycles through them in this order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Table,
    Checks,
    Comments,
    Description,
    Diff,
}

impl Pane {
    fn next(self) -> Self {
        match self {
            Pane::Table => Pane::Checks,
            Pane::Checks => Pane::Comments,
            Pane::Comments => Pane::Description,
            Pane::Description => Pane::Diff,
            Pane::Diff => Pane::Table,
        }
    }

    fn previous(self) -> Self {
        match self {
            Pane::Table => Pane::Diff,
            Pane::Checks => Pane::Table,
            Pane::Comments => Pane::Checks,
            Pane::Description => Pane::Comments,
            Pane::Diff => Pane::Description,
        }
    }
}

pub enum Mode {
    /// No list has been fetched yet.
    Loading,
    List,
    /// Asking what to do with the selected PRs.
    Confirm(Confirm),
    /// Asking before opening many PRs in the browser.
    ConfirmOpen(ConfirmOpen),
    /// Approving PRs.
    Running(Batch),
    /// Showing how a batch went.
    Results(Summary),
}

/// Approves PRs, all at once up to a limit, then merges the approved ones one at a time if
/// asked to.
pub struct Batch {
    pub items: Vec<BatchItem>,
    /// Whether approved PRs are merged afterwards.
    pub merge: bool,
}

pub struct BatchItem {
    pub pr: ConfirmPr,
    pub approval: ItemStatus,
    /// Set once the PR is approved and queued for merging.
    pub merging: Option<ItemStatus>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ItemStatus {
    Pending,
    Working,
    Done,
    /// Auto-merge was turned on, so GitHub merges the PR once its checks pass.
    AutoMerge,
    /// The error, on one line.
    Failed(String),
}

impl ItemStatus {
    pub fn is_finished(&self) -> bool {
        !matches!(self, Self::Pending | Self::Working)
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Done | Self::AutoMerge)
    }
}

impl Batch {
    /// Whether the batch has moved on to merging.
    pub fn is_merging(&self) -> bool {
        self.items.iter().any(|item| item.merging.is_some())
    }

    /// How many items of the current step are finished, out of how many.
    pub fn progress(&self) -> (usize, usize) {
        let statuses: Vec<&ItemStatus> = if self.is_merging() {
            self.items
                .iter()
                .filter_map(|item| item.merging.as_ref())
                .collect()
        } else {
            self.items.iter().map(|item| &item.approval).collect()
        };
        let finished = statuses
            .iter()
            .filter(|status| status.is_finished())
            .count();
        (finished, statuses.len())
    }
}

#[derive(Debug)]
pub struct Summary {
    pub approved: usize,
    pub total: usize,
    /// How many were merged or queued for auto-merge, out of how many were tried. `None` when
    /// the batch didn't merge.
    pub merged: Option<(usize, usize)>,
    /// PR numbers, the step that failed and the error on one line.
    pub failures: Vec<(u64, &'static str, String)>,
    /// PRs that will merge once their checks pass.
    pub auto_merge: Vec<u64>,
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

/// The dialog that asks before opening more than [`MAX_OPEN_WITHOUT_ASKING`] PRs.
pub struct ConfirmOpen {
    /// The PRs to open, in table order.
    pub numbers: Vec<u64>,
    /// Whether Open, rather than Cancel, is highlighted.
    pub open: bool,
}

pub struct Row {
    pub pr: Pr,
    pub checks: Checks,
}

/// Something fetched in the background for a PR, such as its details or its diff.
pub enum Fetch<T> {
    Loading,
    Loaded(T),
    /// Kept rather than removed, so a failing PR isn't fetched again on every update.
    Failed(String),
}

/// A PR's details, fetched in the background.
pub struct Details {
    pub details: PrDetails,
    /// The body with HTML comments and extra blank lines removed.
    pub body: String,
}

/// A PR's diff with gh's colours.
pub type Diff = Text<'static>;

/// Marks the PRs in `order` that have no entry in `fetches` as loading, and returns them, as
/// many as there are free slots out of `max`. The caller must start their fetches.
fn start_fetches<T>(fetches: &mut HashMap<u64, Fetch<T>>, order: Vec<u64>, max: usize) -> Vec<u64> {
    let in_flight = fetches
        .values()
        .filter(|fetch| matches!(fetch, Fetch::Loading))
        .count();
    let numbers: Vec<u64> = order
        .into_iter()
        .filter(|number| !fetches.contains_key(number))
        .take(max.saturating_sub(in_flight))
        .collect();
    for &number in &numbers {
        fetches.insert(number, Fetch::Loading);
    }
    numbers
}

/// Drops the failed fetches, so they are tried again.
fn forget_failed<T>(fetches: &mut HashMap<u64, Fetch<T>>) {
    fetches.retain(|_, fetch| !matches!(fetch, Fetch::Failed(_)));
}

pub enum Msg {
    Key(KeyEvent),
    /// The result of the list fetch with this id.
    PrsLoaded(u64, Result<Vec<Pr>>),
    DetailsLoaded(u64, Result<PrDetails>),
    DiffLoaded(u64, Result<String>),
    /// The result of approving this PR.
    Approved(u64, Result<()>),
    /// The result of merging this PR, or of turning on auto-merge.
    Merged(u64, Result<()>),
    /// The result of fetching this PR again to refresh its row.
    PrRefreshed(u64, Result<Pr>),
    /// The result of opening this PR in the browser.
    Opened(u64, Result<()>),
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
            focus: Pane::Table,
            checks_scroll: 0,
            description_scroll: 0,
            diff_scroll: 0,
            comment: 0,
            comment_scroll: 0,
            page: 1,
            error: None,
            refresh_errors: Vec::new(),
            open_errors: Vec::new(),
            opens_to_run: Vec::new(),
            selected: HashSet::new(),
            details: HashMap::new(),
            diffs: HashMap::new(),
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

    /// PRs to open in the browser, one after another so the tabs come in table order. The caller
    /// must start them.
    pub fn opens_to_run(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.opens_to_run)
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
            .filter(|item| item.approval == ItemStatus::Working)
            .count();
        batch
            .items
            .iter_mut()
            .filter(|item| item.approval == ItemStatus::Pending)
            .take(MAX_APPROVALS.saturating_sub(working))
            .map(|item| {
                item.approval = ItemStatus::Working;
                item.pr.number
            })
            .collect()
    }

    /// The next merge to start, and whether it should use auto-merge. Merges run one at a time,
    /// so this returns nothing while one is working. Marks it as working, so the caller must
    /// start it.
    pub fn merge_to_run(&mut self) -> Option<(u64, bool)> {
        let Mode::Running(batch) = &mut self.mode else {
            return None;
        };
        if batch
            .items
            .iter()
            .any(|item| item.merging == Some(ItemStatus::Working))
        {
            return None;
        }
        let item = batch
            .items
            .iter_mut()
            .find(|item| item.merging == Some(ItemStatus::Pending))?;
        item.merging = Some(ItemStatus::Working);
        Some((item.pr.number, auto_merge(item.pr.checks)))
    }

    /// A submitted query and the id of its fetch, which the caller must start.
    pub fn list_to_fetch(&mut self) -> Option<(u64, String)> {
        let query = self.query_to_fetch.take()?;
        self.fetch_id += 1;
        self.searching = true;
        Some((self.fetch_id, query))
    }

    /// PRs whose details haven't been requested yet, as many as there are free fetch slots.
    /// Marks them as loading, so the caller must start the fetches.
    pub fn details_to_fetch(&mut self) -> Vec<u64> {
        let order = self.numbers_from_highlight();
        start_fetches(&mut self.details, order, MAX_DETAIL_FETCHES)
    }

    /// PRs whose diffs haven't been requested yet, as many as there are free fetch slots. Marks
    /// them as loading, so the caller must start the fetches.
    pub fn diffs_to_fetch(&mut self) -> Vec<u64> {
        let order = self.numbers_from_highlight();
        start_fetches(&mut self.diffs, order, MAX_DIFF_FETCHES)
    }

    /// The PR numbers in the order background fetches should take them: the highlighted PR, then
    /// the rows below it, then those above. A fetch started as a slot frees up so goes to the
    /// rows about to be viewed.
    fn numbers_from_highlight(&self) -> Vec<u64> {
        let Some(start) = self.table.selected() else {
            return Vec::new();
        };
        (start..self.rows.len())
            .chain(0..start)
            .map(|i| self.rows[i].pr.number)
            .collect()
    }

    /// What to show above the status bar: why the search failed, then the row refreshes that
    /// failed. Only the first few of those are listed, as the space for them is small.
    pub fn error_text(&self) -> Option<String> {
        let mut lines: Vec<String> = self.error.iter().cloned().collect();
        lines.extend(self.refresh_errors.iter().take(MAX_REFRESH_ERRORS).cloned());
        let more = self.refresh_errors.len().saturating_sub(MAX_REFRESH_ERRORS);
        if more > 0 {
            lines.push(format!("… and {more} more refreshes failed"));
        }
        lines.extend(self.open_errors.iter().take(MAX_REFRESH_ERRORS).cloned());
        let more = self.open_errors.len().saturating_sub(MAX_REFRESH_ERRORS);
        if more > 0 {
            lines.push(format!("… and {more} more PRs failed to open"));
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    fn clear_errors(&mut self) {
        self.error = None;
        self.refresh_errors.clear();
        self.open_errors.clear();
    }

    pub fn highlighted(&self) -> Option<&Row> {
        self.table.selected().and_then(|i| self.rows.get(i))
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::Tick => self.spinner = self.spinner.wrapping_add(1),
            Msg::Approved(number, result) => self.approved(number, result),
            Msg::Merged(number, result) => self.merged(number, result),
            Msg::Opened(number, Err(e)) => {
                let message = format!("Opening #{number} failed: {e}");
                self.open_errors.push(one_line(&message));
            }
            Msg::Opened(_, Ok(())) => {}
            Msg::PrRefreshed(number, result) => {
                self.refreshing.remove(&number);
                match result {
                    Ok(pr) => {
                        if let Some(row) = self.rows.iter_mut().find(|row| row.pr.number == number)
                        {
                            row.checks =
                                model::classify_checks(pr.mergeable, &pr.status_check_rollup);
                            row.pr = pr;
                        }
                    }
                    // The row keeps its old state, so say why it didn't change.
                    Err(e) => {
                        let message = format!("Refreshing #{number} failed: {e}");
                        self.refresh_errors.push(one_line(&message));
                    }
                }
            }
            Msg::DetailsLoaded(number, Err(e)) => {
                self.details.insert(number, Fetch::Failed(e.to_string()));
            }
            Msg::DetailsLoaded(number, Ok(details)) => {
                let body = model::clean_body(details.body.as_deref());
                self.details
                    .insert(number, Fetch::Loaded(Details { details, body }));
            }
            Msg::DiffLoaded(number, Err(e)) => {
                self.diffs.insert(number, Fetch::Failed(e.to_string()));
            }
            Msg::DiffLoaded(number, Ok(diff)) => {
                // A tab would be drawn as a single cell. Parsing here rather than when drawing
                // means each diff is parsed once.
                let diff = match diff.replace('\t', "    ").into_text() {
                    Ok(text) => Fetch::Loaded(text),
                    Err(e) => Fetch::Failed(format!("couldn't read the colours: {e}")),
                };
                self.diffs.insert(number, diff);
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
                        let checks = model::classify_checks(pr.mergeable, &pr.status_check_rollup);
                        Row { pr, checks }
                    })
                    .collect();
                // Open PRs first, passing first; closed and merged PRs can't be acted on.
                rows.sort_by_key(|row| (row.pr.state != PrState::Open, row.checks.state.rank()));
                self.table.select((!rows.is_empty()).then_some(0));
                self.reset_scroll();
                self.rows = rows;
                self.selected.clear();
                // Give failed details another chance with the new results.
                forget_failed(&mut self.details);
                forget_failed(&mut self.diffs);
                self.clear_errors();
                self.searching = false;
                self.loaded();
            }
        }
    }

    /// Records an approval's result. After the last one, queues the approved PRs for merging if
    /// asked to, or else finishes the batch.
    fn approved(&mut self, number: u64, result: Result<()>) {
        let Mode::Running(batch) = &mut self.mode else {
            return;
        };
        if let Some(item) = batch.items.iter_mut().find(|item| item.pr.number == number) {
            item.approval = status(result, ItemStatus::Done);
        }
        if !batch.items.iter().all(|item| item.approval.is_finished()) {
            return;
        }
        if batch.merge {
            // Only PRs whose approval succeeded are merged.
            for item in &mut batch.items {
                if item.approval.is_ok() {
                    item.merging = Some(ItemStatus::Pending);
                }
            }
        }
        if !batch.is_merging() {
            self.finish_batch();
        }
    }

    /// Records a merge's result, and finishes the batch after the last one.
    fn merged(&mut self, number: u64, result: Result<()>) {
        let Mode::Running(batch) = &mut self.mode else {
            return;
        };
        if let Some(item) = batch.items.iter_mut().find(|item| item.pr.number == number) {
            let ok = if auto_merge(item.pr.checks) {
                ItemStatus::AutoMerge
            } else {
                ItemStatus::Done
            };
            item.merging = Some(status(result, ok));
        }
        let (finished, total) = batch.progress();
        if finished == total {
            self.finish_batch();
        }
    }

    /// Shows the summary, clears the selection and refreshes the batch's rows so they show
    /// their new states.
    fn finish_batch(&mut self) {
        let Mode::Running(batch) = &self.mode else {
            return;
        };
        let mut failures = Vec::new();
        for item in &batch.items {
            let steps = [
                ("approve", Some(&item.approval)),
                ("merge", item.merging.as_ref()),
            ];
            for (step, status) in steps {
                if let Some(ItemStatus::Failed(e)) = status {
                    failures.push((item.pr.number, step, e.clone()));
                }
            }
        }
        let approved = batch
            .items
            .iter()
            .filter(|item| item.approval.is_ok())
            .count();
        let merges: Vec<&ItemStatus> = batch
            .items
            .iter()
            .filter_map(|item| item.merging.as_ref())
            .collect();
        let merged = batch.merge.then(|| {
            let ok = merges.iter().filter(|status| status.is_ok()).count();
            (ok, merges.len())
        });
        let auto_merge = batch
            .items
            .iter()
            .filter(|item| item.merging == Some(ItemStatus::AutoMerge))
            .map(|item| item.pr.number)
            .collect();
        let summary = Summary {
            approved,
            total: batch.items.len(),
            merged,
            failures,
            auto_merge,
        };
        // Only the batch's rows are refreshed. Running the search again could reorder the list
        // or drop PRs the user still means to work through.
        self.to_refresh
            .extend(batch.items.iter().map(|item| item.pr.number));
        self.mode = Mode::Results(summary);
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
        } else if let Mode::ConfirmOpen(confirm) = &mut self.mode {
            match key.code {
                KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => confirm.open = true,
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => confirm.open = false,
                KeyCode::Enter if confirm.open => {
                    self.opens_to_run = std::mem::take(&mut confirm.numbers);
                    self.mode = Mode::List;
                }
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::List,
                _ => {}
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
                self.clear_errors();
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            self.open_in_browser();
            return;
        }
        match key.code {
            KeyCode::Char('/') => self.editing = true,
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Tab => self.focus = self.focus.next(),
            KeyCode::BackTab => self.focus = self.focus.previous(),
            KeyCode::Down | KeyCode::Char('j') => self.move_focused(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_focused(-1),
            KeyCode::PageDown => self.page_focused(1),
            KeyCode::PageUp => self.page_focused(-1),
            KeyCode::Char(' ') => self.toggle_selected(),
            KeyCode::Enter => self.open_confirm(),
            _ => {}
        }
    }

    /// Opens the selected PRs in the browser, or the highlighted one if none are selected, in
    /// table order. Asks first when there are more than [`MAX_OPEN_WITHOUT_ASKING`].
    fn open_in_browser(&mut self) {
        let numbers: Vec<u64> = if self.selected.is_empty() {
            self.highlighted()
                .map(|row| row.pr.number)
                .into_iter()
                .collect()
        } else {
            self.rows
                .iter()
                .map(|row| row.pr.number)
                .filter(|number| self.selected.contains(number))
                .collect()
        };
        self.open_errors.clear();
        if numbers.len() > MAX_OPEN_WITHOUT_ASKING {
            self.mode = Mode::ConfirmOpen(ConfirmOpen {
                numbers,
                open: true,
            });
        } else {
            self.opens_to_run = numbers;
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
        let merge = match choice {
            Choice::Approve => false,
            Choice::ApproveAndMerge => true,
            Choice::Cancel => return,
        };
        // Errors from earlier no longer apply.
        self.clear_errors();
        let items = confirm
            .prs
            .into_iter()
            .map(|pr| BatchItem {
                pr,
                approval: ItemStatus::Pending,
                merging: None,
            })
            .collect();
        self.mode = Mode::Running(Batch { items, merge });
    }

    fn toggle_selected(&mut self) {
        if let Some(number) = self.highlighted().map(|row| row.pr.number)
            && !self.selected.remove(&number)
        {
            self.selected.insert(number);
        }
    }

    /// Moves the table's highlight, or scrolls the focused pane, by `by` rows.
    fn move_focused(&mut self, by: isize) {
        match self.focus {
            Pane::Table => self.move_highlight(by),
            Pane::Checks => {
                self.checks_scroll = self.checks_scroll.saturating_add_signed(clamp_i16(by));
            }
            Pane::Comments => {
                self.comment = self.comment.saturating_add_signed(by);
                self.comment_scroll = 0;
            }
            Pane::Description => {
                self.description_scroll =
                    self.description_scroll.saturating_add_signed(clamp_i16(by));
            }
            Pane::Diff => {
                self.diff_scroll = self.diff_scroll.saturating_add_signed(clamp_i16(by));
            }
        }
    }

    /// PgUp/PgDn: a page in the focused pane. In the comments pane the arrows pick the comment,
    /// so a page scrolls its preview.
    fn page_focused(&mut self, direction: isize) {
        let by = direction * self.page as isize;
        if self.focus == Pane::Comments {
            self.comment_scroll = self.comment_scroll.saturating_add_signed(clamp_i16(by));
        } else {
            self.move_focused(by);
        }
    }

    fn move_highlight(&mut self, by: isize) {
        if let Some(current) = self.table.selected() {
            let last = self.rows.len().saturating_sub(1);
            let new = current.saturating_add_signed(by).min(last);
            if new != current {
                self.table.select(Some(new));
                self.reset_scroll();
            }
        }
    }

    fn reset_scroll(&mut self) {
        self.checks_scroll = 0;
        self.description_scroll = 0;
        self.diff_scroll = 0;
        self.comment = 0;
        self.comment_scroll = 0;
    }
}

fn clamp_i16(n: isize) -> i16 {
    n.clamp(i16::MIN.into(), i16::MAX.into()) as i16
}

/// Whether a PR with these checks is merged with auto-merge, so GitHub merges it once they
/// pass. Only pending checks count: a PR with no checks is merged straight away.
fn auto_merge(checks: CheckState) -> bool {
    checks == CheckState::Pending
}

/// A finished step's status: `ok` on success, or the error on one line.
fn status(result: Result<()>, ok: ItemStatus) -> ItemStatus {
    match result {
        Ok(()) => ok,
        Err(e) => ItemStatus::Failed(one_line(&e.to_string())),
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
    use crate::gh::ReviewDecision;

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

    fn ctrl_o(app: &mut App) {
        app.update(Msg::Key(KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn ctrl_o_opens_the_highlighted_pr_when_none_are_selected() {
        let mut app = app_with_rows(3);
        press(&mut app, "j");
        ctrl_o(&mut app);
        assert_eq!(app.opens_to_run(), [2]);
        assert!(app.opens_to_run().is_empty());
    }

    #[test]
    fn ctrl_o_opens_the_selected_prs_in_table_order() {
        let mut app = app_with_rows(4);
        // Selected out of order: 1, then 3, then 2.
        press(&mut app, " jj k ");
        ctrl_o(&mut app);
        assert_eq!(app.opens_to_run(), [1, 2, 3]);
    }

    #[test]
    fn ctrl_o_asks_before_opening_more_than_five() {
        let mut app = app_with_rows(6);
        for _ in 0..6 {
            press(&mut app, " j");
        }
        ctrl_o(&mut app);
        assert!(app.opens_to_run().is_empty());
        assert!(matches!(app.mode, Mode::ConfirmOpen(_)));
        // Cancel leaves everything alone.
        press(&mut app, "l");
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::List));
        assert!(app.opens_to_run().is_empty());

        ctrl_o(&mut app);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.opens_to_run(), [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn five_selected_prs_open_without_asking() {
        let mut app = app_with_rows(5);
        for _ in 0..5 {
            press(&mut app, " j");
        }
        ctrl_o(&mut app);
        assert!(matches!(app.mode, Mode::List));
        assert_eq!(app.opens_to_run().len(), 5);
    }

    #[test]
    fn a_pr_that_fails_to_open_is_shown_with_the_error() {
        let mut app = app_with_rows(2);
        app.update(Msg::Opened(2, Err(color_eyre::eyre::eyre!("no browser"))));
        assert_eq!(
            app.error_text().as_deref(),
            Some("Opening #2 failed: no browser")
        );
        app.update(Msg::Opened(1, Ok(())));
        assert_eq!(app.error_text().unwrap().lines().count(), 1);
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
        assert_eq!(
            summary.failures,
            [(3, "approve", "no permission".to_string())]
        );
        assert_eq!(summary.merged, None);
        assert!(app.selected.is_empty());
        // Only the batch's rows are fetched again, not the whole search.
        assert_eq!(app.list_to_fetch(), None);
        assert_eq!(app.refreshes_to_run(), [1, 2, 3]);

        key(&mut app, KeyCode::Char('q'));
        assert!(matches!(app.mode, Mode::List));
        assert!(!app.should_quit);
    }

    /// An open PR whose checks are passing, pending or absent.
    fn pr_with_checks(number: u64, checks: &str) -> Pr {
        let rollup = match checks {
            "passing" => serde_json::json!([{
                "__typename": "CheckRun", "name": "build", "workflowName": "CI",
                "status": "COMPLETED", "conclusion": "SUCCESS", "startedAt": null,
            }]),
            "pending" => serde_json::json!([{
                "__typename": "StatusContext", "context": "ci", "state": "PENDING",
            }]),
            _ => serde_json::json!([]),
        };
        serde_json::from_value(serde_json::json!({
            "number": number,
            "title": "t",
            "state": "OPEN",
            "statusCheckRollup": rollup,
            "mergeable": "MERGEABLE",
            "reviewDecision": "",
        }))
        .unwrap()
    }

    /// Lists PRs 1 to 4 (passing, pending, no checks, pending), selects them all and chooses
    /// Approve & merge.
    fn start_approving_and_merging() -> App {
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        let prs = vec![
            pr_with_checks(1, "passing"),
            pr_with_checks(2, "pending"),
            pr_with_checks(3, "none"),
            pr_with_checks(4, "pending"),
        ];
        app.update(Msg::PrsLoaded(id, Ok(prs)));
        assert_eq!(numbers(&app), [1, 2, 3, 4]);
        press(&mut app, " j j j ");
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Running(_)));
        app
    }

    fn merge(app: &mut App, number: u64, result: Result<()>) {
        app.update(Msg::Merged(number, result));
    }

    #[test]
    fn approve_and_merge_merges_approved_prs_one_at_a_time() {
        let mut app = start_approving_and_merging();
        assert_eq!(app.approvals_to_run(), [1, 2, 3, 4]);
        // Nothing is merged until every approval has finished.
        approve(&mut app, 1, Ok(()));
        assert_eq!(app.merge_to_run(), None);
        approve(&mut app, 2, Ok(()));
        approve(&mut app, 3, Ok(()));
        approve(&mut app, 4, Err(color_eyre::eyre::eyre!("own PR")));

        // Only the approved PRs are merged, one at a time. Pending checks use auto-merge; no
        // checks don't.
        let Mode::Running(batch) = &app.mode else {
            panic!("the batch stopped before merging");
        };
        assert!(batch.is_merging());
        assert_eq!(batch.progress(), (0, 3));
        assert_eq!(app.merge_to_run(), Some((1, false)));
        assert_eq!(app.merge_to_run(), None);
        merge(&mut app, 1, Ok(()));
        assert_eq!(app.merge_to_run(), Some((2, true)));
        merge(&mut app, 2, Ok(()));
        assert_eq!(app.merge_to_run(), Some((3, false)));
        merge(&mut app, 3, Err(color_eyre::eyre::eyre!("not mergeable")));
        assert_eq!(app.merge_to_run(), None);

        let Mode::Results(summary) = &app.mode else {
            panic!("the results aren't shown");
        };
        assert_eq!((summary.approved, summary.total), (3, 4));
        // Auto-merge counts as merged.
        assert_eq!(summary.merged, Some((2, 3)));
        assert_eq!(summary.auto_merge, [2]);
        assert_eq!(
            summary.failures,
            [
                (3, "merge", "not mergeable".to_string()),
                (4, "approve", "own PR".to_string()),
            ]
        );
        assert!(app.selected.is_empty());
        assert_eq!(app.refreshes_to_run(), [1, 2, 3, 4]);
    }

    #[test]
    fn approve_and_merge_with_no_approvals_skips_merging() {
        let mut app = start_approving_and_merging();
        app.approvals_to_run();
        for number in 1..=4 {
            approve(&mut app, number, Err(color_eyre::eyre::eyre!("no")));
        }
        assert_eq!(app.merge_to_run(), None);
        let Mode::Results(summary) = &app.mode else {
            panic!("the results aren't shown");
        };
        assert_eq!(summary.merged, Some((0, 0)));
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
        merged.review_decision = ReviewDecision::Approved;
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
        assert_eq!(app.rows[0].pr.review_decision, ReviewDecision::Approved);
        assert_eq!(app.table.selected(), Some(1));
        // Failed refreshes are reported, and the rows keep their old state.
        assert_eq!(
            app.error_text().as_deref(),
            Some("Refreshing #2 failed: failed\nRefreshing #3 failed: timed out")
        );
        assert_eq!(app.rows[1].pr.state, PrState::Open);
    }

    #[test]
    fn refresh_failures_are_listed_few_at_a_time_after_a_search_failure() {
        let mut app = app_with_rows(5);
        app.to_refresh = (1..=5).collect();
        app.refreshes_to_run();
        app.update(Msg::PrRefreshed(1, Err(color_eyre::eyre::eyre!("a\nb"))));
        for number in 2..=5 {
            app.update(Msg::PrRefreshed(
                number,
                Err(color_eyre::eyre::eyre!("failed")),
            ));
        }
        // A failed search leaves the refresh failures, which are about the rows still shown.
        app.error = Some("Search failed: bad query".to_string());
        let text = app.error_text().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "Search failed: bad query");
        // Each failure takes one line, however many lines the error has.
        assert_eq!(lines[1], "Refreshing #1 failed: a b");
        assert_eq!(lines[4], "… and 2 more refreshes failed");

        // New results make them all stale.
        let (id, _) = {
            app.query_to_fetch = Some(String::new());
            app.list_to_fetch().unwrap()
        };
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
        assert_eq!(app.error_text(), None);
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
    fn prefetches_diffs_from_the_highlight_within_the_limit() {
        let mut app = app_with_rows(8);
        assert_eq!(app.diffs_to_fetch(), [1, 2, 3, 4]);
        // Every slot is busy.
        assert!(app.diffs_to_fetch().is_empty());

        // Freed slots go to the highlighted row and those below it, then wrap to the top.
        app.table.select(Some(5));
        app.update(Msg::DiffLoaded(1, Ok("diff\t".to_string())));
        assert!(matches!(&app.diffs[&1], Fetch::Loaded(d) if d.to_string() == "diff    "));
        assert_eq!(app.diffs_to_fetch(), [6]);
        for number in [2, 3] {
            app.update(Msg::DiffLoaded(number, Ok(String::new())));
        }
        assert_eq!(app.diffs_to_fetch(), [7, 8]);
        app.update(Msg::DiffLoaded(4, Ok(String::new())));
        assert_eq!(app.diffs_to_fetch(), [5]);

        // Nothing is fetched twice, including failures.
        for number in 5..=8 {
            app.update(Msg::DiffLoaded(
                number,
                Err(color_eyre::eyre::eyre!("failed")),
            ));
        }
        assert!(app.diffs_to_fetch().is_empty());
    }

    #[test]
    fn diff_colours_become_styles() {
        let mut app = app_with_rows(1);
        app.update(Msg::DiffLoaded(
            1,
            Ok("\x1b[32m+added\x1b[m\n ctx".to_string()),
        ));
        let Fetch::Loaded(text) = &app.diffs[&1] else {
            panic!("diff should be loaded");
        };
        assert_eq!(text.to_string(), "+added\n ctx");
        assert_eq!(
            text.lines[0].spans[0].style.fg,
            Some(ratatui::style::Color::Green)
        );
    }

    /// Presses Tab until `pane` has focus.
    fn focus(app: &mut App, pane: Pane) {
        for _ in 0..4 {
            if app.focus == pane {
                return;
            }
            key(app, KeyCode::Tab);
        }
        panic!("Tab never reached the pane");
    }

    #[test]
    fn tab_cycles_focus_and_arrows_act_on_the_focused_pane() {
        let mut app = app_with_rows(3);
        app.page = 5;
        assert!(app.focus == Pane::Table);

        // The table moves its highlight; a page is clamped to the last row.
        key(&mut app, KeyCode::Down);
        assert_eq!(app.table.selected(), Some(1));
        key(&mut app, KeyCode::PageDown);
        assert_eq!(app.table.selected(), Some(2));

        key(&mut app, KeyCode::Tab);
        assert!(app.focus == Pane::Checks);
        press(&mut app, "j");
        assert_eq!(app.checks_scroll, 1);

        // In the comments pane the arrows pick the comment, and a page scrolls its preview.
        key(&mut app, KeyCode::Tab);
        assert!(app.focus == Pane::Comments);
        press(&mut app, "jj");
        assert_eq!(app.comment, 2);
        key(&mut app, KeyCode::PageDown);
        assert_eq!((app.comment, app.comment_scroll), (2, 5));
        key(&mut app, KeyCode::PageUp);
        key(&mut app, KeyCode::PageUp);
        assert_eq!(app.comment_scroll, 0);
        press(&mut app, "jk");
        assert_eq!(app.comment, 2);
        key(&mut app, KeyCode::PageDown);
        press(&mut app, "k");
        assert_eq!((app.comment, app.comment_scroll), (1, 0));

        key(&mut app, KeyCode::Tab);
        assert!(app.focus == Pane::Description);
        press(&mut app, "jj");
        key(&mut app, KeyCode::PageDown);
        assert_eq!(app.description_scroll, 7);
        key(&mut app, KeyCode::PageUp);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.description_scroll, 1);
        // The highlight stayed where it was, and the other panes didn't scroll.
        assert_eq!(app.table.selected(), Some(2));
        assert_eq!((app.checks_scroll, app.diff_scroll), (1, 0));
        key(&mut app, KeyCode::PageUp);
        assert_eq!(app.description_scroll, 0);

        key(&mut app, KeyCode::Tab);
        assert!(app.focus == Pane::Diff);
        press(&mut app, "k");
        key(&mut app, KeyCode::Down);
        assert_eq!(app.diff_scroll, 1);

        key(&mut app, KeyCode::Tab);
        assert!(app.focus == Pane::Table);
        key(&mut app, KeyCode::BackTab);
        assert!(app.focus == Pane::Diff);
    }

    #[test]
    fn scroll_goes_back_to_the_top_when_the_highlight_moves() {
        let mut app = app_with_rows(3);
        for (pane, keys) in [
            (Pane::Checks, "j"),
            (Pane::Comments, "jj"),
            (Pane::Description, "jj"),
            (Pane::Diff, "jjj"),
        ] {
            focus(&mut app, pane);
            press(&mut app, keys);
        }
        focus(&mut app, Pane::Table);
        key(&mut app, KeyCode::Down);
        assert_eq!(
            (app.checks_scroll, app.description_scroll, app.diff_scroll),
            (0, 0, 0)
        );
        assert_eq!((app.comment, app.comment_scroll), (0, 0));

        // A highlight that can't move, at the last row, leaves the scroll alone.
        key(&mut app, KeyCode::Down);
        focus(&mut app, Pane::Description);
        press(&mut app, "j");
        focus(&mut app, Pane::Table);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.description_scroll, 1);
    }

    #[test]
    fn title_shows_repo_and_dry_run() {
        assert_eq!(title(None, false), "PRs");
        assert_eq!(title(Some("o/r"), false), "PRs (o/r)");
        assert_eq!(title(Some("o/r"), true), "PRs (o/r) [DRY RUN]");
    }
}
