use std::collections::{HashMap, HashSet};

use ansi_to_tui::IntoText;
use color_eyre::eyre::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Text;
use ratatui::widgets::TableState;
use regex::Regex;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::batch::{
    Batch, BatchItem, Choice, Confirm, ConfirmPr, ItemStatus, Summary, auto_merge, finished_status,
    on_confirm_key,
};
use crate::gh::{Pr, PrDetails, PrState, StatusUpdate};
use crate::model::{self, CheckState, Checks};

/// The most detail fetches to run at once.
const MAX_DETAIL_FETCHES: usize = 4;

/// The most diffs fetched at once.
const MAX_DIFF_FETCHES: usize = 4;

/// The most approvals to run at once.
const MAX_APPROVALS: usize = 4;

/// The most failed row refreshes, and the most failed opens, to list above the status bar.
const MAX_LISTED_ERRORS: usize = 3;

/// The most row refreshes to run at once.
const MAX_REFRESHES: usize = 4;

/// The most PRs Ctrl-O opens without asking first.
const MAX_OPEN_WITHOUT_ASKING: usize = 5;

pub struct App {
    /// The title without the note about a truncated list: see [`App::title`].
    title: String,
    /// The most PRs to show. GitHub is asked for one more, so that a list that is cut short can
    /// say so.
    limit: usize,
    /// Whether the latest list had more PRs than `limit`.
    truncated: bool,
    /// The `--column-regex`, whose groups are shown in place of the title.
    pub columns: Option<Regex>,
    pub dry_run: bool,
    pub mode: Mode,
    /// The search box. While it isn't being edited it holds the last submitted query.
    pub query: Input,
    /// The query to restore when an edit is cancelled.
    submitted_query: String,
    pub editing: bool,
    /// Fetching the list.
    pub list: ListFetch,
    /// The timed status refresh.
    pub status: StatusRefresh,
    /// Bumped when a batch starts and when it finishes, so a status refresh that began before
    /// can't put old state back over a row the batch changed.
    batch_epoch: u64,
    pub rows: Vec<Row>,
    /// The indices into `rows` that the filter shows, in order. It is worked out when the filter
    /// changes or a list loads, not when a row is refreshed, so a row whose status changes
    /// doesn't vanish from under the cursor.
    pub visible: Vec<usize>,
    /// The local filter box.
    pub filter: Input,
    /// Whether the filter box is being edited.
    pub filtering: bool,
    /// The highlighted row, as a position in `visible`.
    pub table: TableState,
    /// The pane that Up/Down and PgUp/PgDn act on.
    pub focus: Pane,
    /// How far the panes are scrolled, and the comment picked.
    pub scroll: Scroll,
    /// The rows a PgUp/PgDn moves in the focused pane, set by the view as it draws.
    pub page: u16,
    /// The numbers of the PRs marked with Space.
    pub selected: HashSet<u64>,
    /// What to show above the status bar.
    pub errors: Errors,
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

/// The state of fetching the list of PRs.
#[derive(Default)]
pub struct ListFetch {
    /// A query submitted but not yet fetched.
    query_to_fetch: Option<String>,
    /// Whether `query_to_fetch` is a Ctrl-R refresh rather than a new search.
    refresh_requested: bool,
    /// Whether the latest fetch is a refresh. A refresh keeps the selection.
    is_refresh: bool,
    /// The query of the latest fetch.
    fetching_query: String,
    /// The query of the list shown. The timed refresh uses it, as a search that failed left the
    /// older list in place.
    shown_query: String,
    /// The id of the latest fetch. Results from older fetches are dropped.
    id: u64,
    /// Whether the latest fetch is still running.
    pub searching: bool,
}

/// The timed status refresh.
#[derive(Default)]
pub struct StatusRefresh {
    /// Set when it's time for a refresh, until [`App::status_to_fetch`] starts it.
    requested: bool,
    /// Whether a refresh is running.
    in_flight: bool,
    /// Why the last refresh failed, shown in the status bar until one succeeds.
    pub failure: Option<String>,
}

/// Rows scrolled off the top of the checks, description and diff panes. They go back to the top
/// when the highlight moves. The view clamps them to the content it draws.
#[derive(Default)]
pub struct Scroll {
    pub checks: u16,
    pub description: u16,
    pub diff: u16,
    /// The selected comment of the highlighted PR. The view clamps it to the comments there are.
    pub comment: usize,
    /// Rows scrolled off the top of the selected comment's preview.
    pub comment_scroll: u16,
}

/// The errors shown above the status bar.
#[derive(Default)]
pub struct Errors {
    /// Why the last search failed.
    search: Option<String>,
    /// One line for each row refresh that failed, shown after `search`.
    refreshes: Vec<String>,
    /// One line for each PR that failed to open in the browser, shown after `refreshes`.
    opens: Vec<String>,
}

impl Errors {
    /// Why the search failed, then the row refreshes and opens that failed. Only the first few
    /// of those are listed, as the space for them is small.
    pub fn text(&self) -> Option<String> {
        let mut lines: Vec<String> = self.search.iter().cloned().collect();
        push_listed(&mut lines, &self.refreshes, "refreshes failed");
        push_listed(&mut lines, &self.opens, "PRs failed to open");
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Adds the first [`MAX_LISTED_ERRORS`] of `errors` to `lines`, then a line counting the rest as
/// "… and N more `what`".
fn push_listed(lines: &mut Vec<String>, errors: &[String], what: &str) {
    lines.extend(errors.iter().take(MAX_LISTED_ERRORS).cloned());
    let more = errors.len().saturating_sub(MAX_LISTED_ERRORS);
    if more > 0 {
        lines.push(format!("… and {more} more {what}"));
    }
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
    /// Listing every key.
    Help,
    /// Approving PRs.
    Running(Batch),
    /// Showing how a batch went.
    Results(Summary),
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

impl Row {
    /// The checks column as drawn: the checks of a closed or merged PR no longer matter.
    pub fn checks_label(&self) -> &'static str {
        match self.pr.state {
            PrState::Open => self.checks.state.label(),
            PrState::Closed => "· closed",
            PrState::Merged => "· merged",
        }
    }

    /// Whether any visible column contains `text`, which must be lowercase.
    fn matches(&self, text: &str, columns: Option<&Regex>) -> bool {
        [
            format!("#{}", self.pr.number),
            self.checks_label().to_string(),
            model::review_label(self.pr.review_decision).to_string(),
        ]
        .into_iter()
        .chain(self.title_cells(columns))
        .any(|column| column.to_lowercase().contains(text))
    }

    /// The cells that replace the title in the table: the title itself, or what each group of
    /// the `--column-regex` matched in it.
    pub fn title_cells(&self, columns: Option<&Regex>) -> Vec<String> {
        match columns {
            Some(regex) => model::column_cells(regex, &self.pr.title),
            None => vec![self.pr.title.clone()],
        }
    }
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

/// What a timed status refresh was started for. Its results are dropped if either has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshId {
    /// The id of the list fetch it followed.
    list_id: u64,
    /// The batch state: see [`App::batch_epoch`].
    batch_epoch: u64,
}

/// A timed status refresh for the caller to start.
pub struct StatusFetch {
    pub id: RefreshId,
    /// The query of the list shown.
    pub query: String,
    /// The open PRs shown, to look up one by one any that the query no longer lists.
    pub candidates: Vec<u64>,
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
    /// Time for a timed status refresh.
    RefreshDue,
    /// The result of the timed status refresh with this id.
    StatusRefreshed(RefreshId, StatusUpdate),
}

impl App {
    pub fn new(title: String, query: String, dry_run: bool) -> Self {
        Self {
            title,
            limit: usize::MAX,
            truncated: false,
            columns: None,
            dry_run,
            mode: Mode::Loading,
            query: Input::new(query.clone()),
            submitted_query: query.clone(),
            editing: false,
            list: ListFetch {
                query_to_fetch: Some(query),
                ..ListFetch::default()
            },
            status: StatusRefresh::default(),
            batch_epoch: 0,
            rows: Vec::new(),
            visible: Vec::new(),
            filter: Input::default(),
            filtering: false,
            table: TableState::default(),
            focus: Pane::Table,
            scroll: Scroll::default(),
            page: 1,
            errors: Errors::default(),
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

    /// Shows at most `limit` PRs, and says when there are more.
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = limit as usize;
        self
    }

    /// Shows a column for each named group of `columns` in place of the title.
    pub fn with_columns(mut self, columns: Option<Regex>) -> Self {
        self.columns = columns;
        self
    }

    /// The title, with a note when the list was cut short.
    pub fn title(&self) -> String {
        if self.truncated {
            format!("{} - showing first {}, more exist", self.title, self.limit)
        } else {
            self.title.clone()
        }
    }

    /// Whether the spinner needs ticks.
    pub fn animating(&self) -> bool {
        self.list.searching || self.refreshing() || matches!(self.mode, Mode::Running(_))
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
        let query = self.list.query_to_fetch.take()?;
        self.list.is_refresh = std::mem::take(&mut self.list.refresh_requested);
        self.list.fetching_query = query.clone();
        self.list.id += 1;
        self.list.searching = true;
        Some((self.list.id, query))
    }

    /// A timed status refresh to start, if one is due. The caller must start it, and send back
    /// [`Msg::StatusRefreshed`] with its id.
    pub fn status_to_fetch(&mut self) -> Option<StatusFetch> {
        if !std::mem::take(&mut self.status.requested) {
            return None;
        }
        // Closed and merged PRs are never refreshed again.
        let candidates: Vec<u64> = self
            .rows
            .iter()
            .filter(|row| row.pr.state == PrState::Open)
            .map(|row| row.pr.number)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        self.status.in_flight = true;
        Some(StatusFetch {
            id: self.refresh_id(),
            query: self.list.shown_query.clone(),
            candidates,
        })
    }

    /// What a status refresh started now would be tied to: the list and the batch state it saw.
    fn refresh_id(&self) -> RefreshId {
        RefreshId {
            list_id: self.list.id,
            batch_epoch: self.batch_epoch,
        }
    }

    /// Whether a timed refresh can start now. It runs in every mode, but not while a batch runs,
    /// another refresh or a search is in flight, or before there is a list.
    fn can_refresh(&self) -> bool {
        !self.rows.is_empty()
            && !self.list.searching
            && self.list.query_to_fetch.is_none()
            && !self.status.in_flight
            && !self.refreshing()
            && !matches!(self.mode, Mode::Running(_))
    }

    /// Updates the rows' status from a timed refresh, in place: no row is added, removed or
    /// moved, and the filter isn't applied again, so a row that no longer matches it stays put.
    /// A PR that changed since the list loaded has its details and diff fetched again.
    fn apply_statuses(&mut self, update: StatusUpdate) {
        for status in update.statuses {
            let number = status.number;
            let Some(row) = self.rows.iter_mut().find(|row| row.pr.number == number) else {
                continue;
            };
            // A PR known to be closed or merged is never refreshed again.
            if row.pr.state != PrState::Open {
                continue;
            }
            if row.pr.updated_at != status.updated_at {
                self.details.remove(&number);
                self.diffs.remove(&number);
            }
            row.checks = model::classify_checks(status.mergeable, &status.status_check_rollup);
            row.pr.update(status);
        }
        self.status.failure = update.failure.map(|failure| one_line(&failure));
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
    /// the rows below it, then those above. A fetch that starts when a slot frees up goes to the
    /// rows about to be viewed.
    fn numbers_from_highlight(&self) -> Vec<u64> {
        // Rows the filter hides are fetched too.
        let Some(start) = self.highlighted_index() else {
            return Vec::new();
        };
        (start..self.rows.len())
            .chain(0..start)
            .map(|i| self.rows[i].pr.number)
            .collect()
    }

    /// The index in `rows` of the highlighted row.
    fn highlighted_index(&self) -> Option<usize> {
        self.table
            .selected()
            .and_then(|i| self.visible.get(i))
            .copied()
    }

    pub fn highlighted(&self) -> Option<&Row> {
        self.highlighted_index().map(|i| &self.rows[i])
    }

    /// The rows the filter shows, in order.
    pub fn visible_rows(&self) -> impl Iterator<Item = &Row> {
        self.visible.iter().map(|&i| &self.rows[i])
    }

    /// How many selected PRs the filter hides.
    pub fn hidden_selected(&self) -> usize {
        let visible = self
            .visible_rows()
            .filter(|row| self.selected.contains(&row.pr.number));
        self.selected.len() - visible.count()
    }

    /// Works out `visible` from the filter. The highlighted PR stays highlighted if it is still
    /// visible, else the first row is.
    fn refilter(&mut self) {
        let before = self.highlighted().map(|row| row.pr.number);
        let text = self.filter.value().to_lowercase();
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.matches(&text, self.columns.as_ref()))
            .map(|(i, _)| i)
            .collect();
        let position = before.and_then(|number| {
            self.visible
                .iter()
                .position(|&i| self.rows[i].pr.number == number)
        });
        self.table
            .select(position.or((!self.visible.is_empty()).then_some(0)));
        if self.highlighted().map(|row| row.pr.number) != before {
            self.scroll = Scroll::default();
        }
    }

    fn clear_filter(&mut self) {
        self.filter = Input::default();
        self.filtering = false;
        self.refilter();
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::Tick => self.spinner = self.spinner.wrapping_add(1),
            Msg::RefreshDue => self.status.requested = self.can_refresh(),
            Msg::StatusRefreshed(id, update) => {
                self.status.in_flight = false;
                // Results for a list or batch state that has since changed are dropped.
                if id == self.refresh_id() {
                    self.apply_statuses(update);
                }
            }
            Msg::Approved(number, result) => self.approved(number, result),
            Msg::Merged(number, result) => self.merged(number, result),
            Msg::Opened(number, Err(e)) => {
                let message = format!("Opening #{number} failed: {e}");
                self.errors.opens.push(one_line(&message));
            }
            Msg::Opened(_, Ok(())) => {}
            Msg::PrRefreshed(number, result) => self.row_refreshed(number, result),
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
            Msg::PrsLoaded(id, _) if id != self.list.id => {}
            Msg::PrsLoaded(_, Err(e)) => {
                // The previous results stay.
                self.errors.search = Some(format!("Search failed: {e}"));
                self.list.searching = false;
                self.loaded();
            }
            Msg::PrsLoaded(_, Ok(prs)) => self.list_loaded(prs),
        }
    }

    /// Replaces the list with the latest fetch's PRs. A new search starts afresh; a refresh
    /// (Ctrl-R) keeps the highlight and the selection on the same PRs where they are still listed.
    fn list_loaded(&mut self, mut prs: Vec<Pr>) {
        // GitHub was asked for one more than the limit, to tell whether it was reached.
        self.truncated = prs.len() > self.limit;
        prs.truncate(self.limit);
        let mut rows: Vec<Row> = prs
            .into_iter()
            .map(|pr| {
                let checks = model::classify_checks(pr.mergeable, &pr.status_check_rollup);
                Row { pr, checks }
            })
            .collect();
        // Open PRs first, passing first; closed and merged PRs can't be acted on.
        rows.sort_by_key(|row| (row.pr.state != PrState::Open, row.checks.state.rank()));
        let before = self.highlighted().map(|row| row.pr.number);
        // A refresh keeps the highlight on the same PR if it is still listed.
        let highlight = before
            .filter(|_| self.list.is_refresh)
            .and_then(|number| rows.iter().position(|row| row.pr.number == number))
            .or((!rows.is_empty()).then_some(0));
        self.table.select(highlight);
        if !self.list.is_refresh || highlight.map(|i| rows[i].pr.number) != before {
            self.scroll = Scroll::default();
        }
        self.visible = (0..rows.len()).collect();
        self.rows = rows;
        self.list.shown_query = self.list.fetching_query.clone();
        // The filter is cleared either way.
        self.filter = Input::default();
        self.filtering = false;
        if self.list.is_refresh {
            // The selection is kept by PR number, so a PR that moved is still selected.
            // One that is no longer listed is dropped.
            let listed: HashSet<u64> = self.rows.iter().map(|row| row.pr.number).collect();
            self.selected.retain(|number| listed.contains(number));
            // The panes show current data, so nothing fetched earlier is kept.
            self.details.clear();
            self.diffs.clear();
        } else {
            self.selected.clear();
            // A PR's details don't depend on the query, so a new search keeps them. Give
            // failed ones another chance.
            forget_failed(&mut self.details);
            forget_failed(&mut self.diffs);
        }
        self.errors.clear();
        self.list.searching = false;
        self.loaded();
    }

    /// Replaces one row with its freshly fetched PR, or says why it couldn't be.
    fn row_refreshed(&mut self, number: u64, result: Result<Pr>) {
        self.refreshing.remove(&number);
        match result {
            Ok(pr) => {
                if let Some(row) = self.rows.iter_mut().find(|row| row.pr.number == number) {
                    row.checks = model::classify_checks(pr.mergeable, &pr.status_check_rollup);
                    row.pr = pr;
                }
            }
            // The row keeps its old state, so say why it didn't change.
            Err(e) => {
                let message = format!("Refreshing #{number} failed: {e}");
                self.errors.refreshes.push(one_line(&message));
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
            item.approval = finished_status(result, ItemStatus::Done);
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
            item.merging = Some(finished_status(result, ok));
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
        let summary = batch.summary();
        // Only the batch's rows are refreshed. Running the search again could reorder the list
        // or drop PRs the user still means to work through.
        self.to_refresh
            .extend(batch.items.iter().map(|item| item.pr.number));
        self.batch_epoch += 1;
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
            return;
        }
        match &mut self.mode {
            Mode::Confirm(confirm) => {
                if let Some(choice) = on_confirm_key(confirm, key) {
                    self.choose(choice);
                }
            }
            Mode::ConfirmOpen(confirm) => match key.code {
                KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => confirm.open = true,
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => confirm.open = false,
                KeyCode::Enter if confirm.open => {
                    self.opens_to_run = std::mem::take(&mut confirm.numbers);
                    self.mode = Mode::List;
                }
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::List,
                _ => {}
            },
            Mode::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?' | 'q')) {
                    self.mode = Mode::List;
                }
            }
            Mode::Running(batch) => {
                if key.code == KeyCode::Char('q') {
                    batch.cancel();
                    self.finish_batch();
                }
            }
            Mode::Results(_) => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
                    self.mode = Mode::List;
                }
            }
            Mode::Loading | Mode::List => {
                if self.editing {
                    self.on_search_key(key);
                } else if self.filtering {
                    self.on_filter_key(key);
                } else {
                    self.on_list_key(key);
                }
            }
        }
    }

    /// While editing, every key other than Enter and Esc goes to the search box.
    fn on_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.editing = false;
                self.submitted_query = self.query.value().to_string();
                self.list.query_to_fetch = Some(self.submitted_query.clone());
                self.list.refresh_requested = false;
                self.errors.clear();
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

    /// While filtering, every key other than Enter and Esc goes to the filter box, and the list
    /// is filtered as you type. Enter keeps the filter; Esc clears it.
    fn on_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.filtering = false,
            KeyCode::Esc => self.clear_filter(),
            _ => {
                self.filter.handle_event(&Event::Key(key));
                self.refilter();
            }
        }
    }

    fn on_list_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('o') => self.open_in_browser(),
                KeyCode::Char('a') => self.select_passing(),
                KeyCode::Char('r') => self.refresh_list(),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('/') => self.editing = true,
            KeyCode::Char('f') => self.filtering = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('q') => self.should_quit = true,
            // Esc never quits: it is too easy to hit by accident. It clears the filter, then
            // the selection.
            KeyCode::Esc if !self.filter.value().is_empty() => self.clear_filter(),
            KeyCode::Esc => self.selected.clear(),
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

    /// The selected rows in table order, or the highlighted row if none are selected.
    fn targets(&self) -> Vec<&Row> {
        if self.selected.is_empty() {
            self.highlighted().into_iter().collect()
        } else {
            self.rows
                .iter()
                .filter(|row| self.selected.contains(&row.pr.number))
                .collect()
        }
    }

    /// Opens the selected PRs in the browser, or the highlighted one if none are selected, in
    /// table order. Asks first when there are more than [`MAX_OPEN_WITHOUT_ASKING`].
    fn open_in_browser(&mut self) {
        let numbers: Vec<u64> = self.targets().iter().map(|row| row.pr.number).collect();
        self.errors.opens.clear();
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
        let rows = self.targets();
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
        self.errors.clear();
        let items = confirm
            .prs
            .into_iter()
            .map(|pr| BatchItem {
                pr,
                approval: ItemStatus::Pending,
                merging: None,
            })
            .collect();
        self.batch_epoch += 1;
        self.mode = Mode::Running(Batch { items, merge });
    }

    /// Runs the submitted query again and repopulates everything, keeping the selection. Does
    /// nothing while a search is running.
    fn refresh_list(&mut self) {
        if self.list.searching || self.list.query_to_fetch.is_some() {
            return;
        }
        self.list.query_to_fetch = Some(self.submitted_query.clone());
        self.list.refresh_requested = true;
        self.errors.clear();
    }

    /// Selects every visible open PR whose checks pass, keeping what is already selected.
    /// Nothing is approved.
    fn select_passing(&mut self) {
        let passing: Vec<u64> = self
            .visible_rows()
            .filter(|row| row.pr.state == PrState::Open && row.checks.state == CheckState::Passing)
            .map(|row| row.pr.number)
            .collect();
        self.selected.extend(passing);
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
                self.scroll.checks = self.scroll.checks.saturating_add_signed(clamp_i16(by));
            }
            Pane::Comments => {
                self.scroll.comment = self.scroll.comment.saturating_add_signed(by);
                self.scroll.comment_scroll = 0;
            }
            Pane::Description => {
                self.scroll.description =
                    self.scroll.description.saturating_add_signed(clamp_i16(by));
            }
            Pane::Diff => {
                self.scroll.diff = self.scroll.diff.saturating_add_signed(clamp_i16(by));
            }
        }
    }

    /// PgUp/PgDn: a page in the focused pane. In the comments pane the arrows pick the comment,
    /// so a page scrolls its preview.
    fn page_focused(&mut self, direction: isize) {
        let by = direction * self.page as isize;
        if self.focus == Pane::Comments {
            self.scroll.comment_scroll = self
                .scroll
                .comment_scroll
                .saturating_add_signed(clamp_i16(by));
        } else {
            self.move_focused(by);
        }
    }

    fn move_highlight(&mut self, by: isize) {
        if let Some(current) = self.table.selected() {
            let last = self.visible.len().saturating_sub(1);
            let new = current.saturating_add_signed(by).min(last);
            if new != current {
                self.table.select(Some(new));
                self.scroll = Scroll::default();
            }
        }
    }
}

fn clamp_i16(n: isize) -> i16 {
    n.clamp(i16::MIN.into(), i16::MAX.into()) as i16
}

/// Joins the lines of `text` with spaces, so it fits in one row.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
    use crate::gh::{PrStatus, ReviewDecision};

    fn pr(number: u64, state: &str) -> Pr {
        serde_json::from_value(serde_json::json!({
            "number": number,
            "title": "t",
            "state": state,
            "statusCheckRollup": [],
            "mergeable": "MERGEABLE",
            "reviewDecision": "",
            "updatedAt": "2026-01-01T00:00:00Z",
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
    fn esc_in_the_list_clears_the_selection_and_never_quits() {
        let mut app = app_with_rows(3);
        press(&mut app, " j ");
        assert_eq!(app.selected.len(), 2);
        key(&mut app, KeyCode::Esc);
        assert!(app.selected.is_empty());
        key(&mut app, KeyCode::Esc);
        assert!(!app.should_quit);
        press(&mut app, "q");
        assert!(app.should_quit);
    }

    #[test]
    fn help_opens_with_question_mark_and_closes_again() {
        let mut app = app_with_rows(3);
        for close in [KeyCode::Esc, KeyCode::Char('?'), KeyCode::Char('q')] {
            press(&mut app, "?");
            assert!(matches!(app.mode, Mode::Help));
            key(&mut app, close);
            assert!(matches!(app.mode, Mode::List));
        }
        // `q` closed the popup rather than quitting.
        assert!(!app.should_quit);
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
        assert!(app.list.searching);

        app.update(Msg::PrsLoaded(id, Ok(prs([7, 8]))));
        assert!(!app.list.searching);
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

    fn titled(number: u64, title: &str) -> Pr {
        let mut pr = pr(number, "OPEN");
        pr.title = title.to_string();
        pr
    }

    /// Three open PRs titled "Bump alpha", "Bump beta" and "Update gamma".
    fn app_with_titles() -> App {
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        let prs = vec![
            titled(1, "Bump alpha"),
            titled(2, "Bump beta"),
            titled(3, "Update gamma"),
        ];
        app.update(Msg::PrsLoaded(id, Ok(prs)));
        app
    }

    fn visible_numbers(app: &App) -> Vec<u64> {
        app.visible_rows().map(|row| row.pr.number).collect()
    }

    #[test]
    fn the_filter_matches_each_visible_column_without_case() {
        let mut app = app_with_titles();
        press(&mut app, "fBUMP");
        assert_eq!(visible_numbers(&app), [1, 2]);
        for (text, expected) in [
            ("#3", vec![3]),
            ("no checks", vec![1, 2, 3]),
            ("needed", vec![1, 2, 3]),
            ("approved", vec![]),
        ] {
            key(&mut app, KeyCode::Esc);
            press(&mut app, "f");
            press(&mut app, text);
            assert_eq!(visible_numbers(&app), expected, "{text}");
        }
    }

    #[test]
    fn the_filter_matches_closed_and_merged_as_drawn() {
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(
            id,
            Ok(vec![pr(1, "OPEN"), pr(2, "MERGED"), pr(3, "CLOSED")]),
        ));
        press(&mut app, "fmerged");
        assert_eq!(visible_numbers(&app), [2]);
    }

    #[test]
    fn the_highlight_stays_on_its_pr_while_it_is_visible() {
        let mut app = app_with_titles();
        press(&mut app, "jf");
        assert_eq!(app.highlighted().unwrap().pr.number, 2);
        // Row 2 is now the first visible row, but still highlighted.
        press(&mut app, "beta");
        assert_eq!(visible_numbers(&app), [2]);
        assert_eq!(app.highlighted().unwrap().pr.number, 2);
        // Once it is hidden, the first visible row is highlighted.
        key(&mut app, KeyCode::Backspace);
        key(&mut app, KeyCode::Backspace);
        key(&mut app, KeyCode::Backspace);
        key(&mut app, KeyCode::Backspace);
        press(&mut app, "gamma");
        assert_eq!(app.highlighted().unwrap().pr.number, 3);
    }

    #[test]
    fn moving_stays_within_the_visible_rows() {
        let mut app = app_with_titles();
        press(&mut app, "fbump");
        key(&mut app, KeyCode::Enter);
        press(&mut app, "jjj");
        assert_eq!(app.highlighted().unwrap().pr.number, 2);
        press(&mut app, "kkk");
        assert_eq!(app.highlighted().unwrap().pr.number, 1);
    }

    #[test]
    fn enter_keeps_the_filter_and_esc_clears_it_before_the_selection() {
        let mut app = app_with_titles();
        press(&mut app, " fgamma");
        key(&mut app, KeyCode::Enter);
        assert!(!app.filtering);
        assert_eq!(visible_numbers(&app), [3]);
        // Esc in the list clears the filter first and leaves the selection.
        key(&mut app, KeyCode::Esc);
        assert_eq!(visible_numbers(&app), [1, 2, 3]);
        assert_eq!(app.selected, HashSet::from([1]));
        key(&mut app, KeyCode::Esc);
        assert!(app.selected.is_empty());
    }

    #[test]
    fn esc_in_the_box_clears_the_filter() {
        let mut app = app_with_titles();
        press(&mut app, "fgamma");
        key(&mut app, KeyCode::Esc);
        assert!(!app.filtering);
        assert_eq!(app.filter.value(), "");
        assert_eq!(visible_numbers(&app), [1, 2, 3]);
    }

    #[test]
    fn the_filter_box_takes_every_key_while_editing() {
        let mut app = app_with_titles();
        press(&mut app, "fq j/");
        assert!(!app.should_quit);
        assert!(!app.editing);
        assert_eq!(app.filter.value(), "q j/");
    }

    #[test]
    fn hidden_selected_prs_stay_selected_and_are_reviewed() {
        let mut app = app_with_titles();
        press(&mut app, " jj ");
        assert_eq!(app.selected, HashSet::from([1, 3]));
        press(&mut app, "fbeta");
        key(&mut app, KeyCode::Enter);
        assert_eq!(visible_numbers(&app), [2]);
        assert_eq!(app.selected, HashSet::from([1, 3]));
        assert_eq!(app.hidden_selected(), 2);

        key(&mut app, KeyCode::Enter);
        assert_eq!(confirm_numbers(&app), [1, 3]);
    }

    #[test]
    fn a_new_search_clears_the_filter() {
        let mut app = app_with_titles();
        press(&mut app, "fgamma");
        key(&mut app, KeyCode::Enter);
        press(&mut app, "/");
        key(&mut app, KeyCode::Enter);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(vec![titled(7, "Bump alpha")])));
        assert_eq!(app.filter.value(), "");
        assert_eq!(visible_numbers(&app), [7]);
    }

    #[test]
    fn a_refreshed_row_stays_where_it_was_even_if_it_no_longer_matches() {
        let mut app = app_with_titles();
        press(&mut app, "falpha");
        assert_eq!(visible_numbers(&app), [1]);
        app.update(Msg::PrRefreshed(1, Ok(titled(1, "Something else"))));
        assert_eq!(visible_numbers(&app), [1]);
        assert_eq!(app.highlighted().unwrap().pr.title, "Something else");
    }

    #[test]
    fn rows_the_filter_hides_are_still_prefetched() {
        let mut app = app_with_titles();
        press(&mut app, "fgamma");
        assert_eq!(app.details_to_fetch(), [3, 1, 2]);
    }

    fn ctrl_a(app: &mut App) {
        app.update(Msg::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn ctrl_a_selects_open_prs_with_passing_checks_only() {
        let mut closed = pr_with_checks(4, "passing");
        closed.state = PrState::Merged;
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        let prs = vec![
            pr_with_checks(1, "passing"),
            pr_with_checks(2, "pending"),
            pr_with_checks(3, "none"),
            closed,
            pr_with_checks(5, "passing"),
        ];
        app.update(Msg::PrsLoaded(id, Ok(prs)));
        ctrl_a(&mut app);
        assert_eq!(app.selected, HashSet::from([1, 5]));
        // Nothing starts: it only selects.
        assert!(matches!(app.mode, Mode::List));
    }

    #[test]
    fn ctrl_a_adds_to_the_selection_and_only_from_visible_rows() {
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        let mut first = pr_with_checks(1, "passing");
        first.title = "keep".to_string();
        let mut second = pr_with_checks(2, "passing");
        second.title = "other".to_string();
        let mut pending = pr_with_checks(3, "pending");
        pending.title = "keep".to_string();
        app.update(Msg::PrsLoaded(id, Ok(vec![first, second, pending])));
        // Row 3 is pending, so Ctrl-A won't pick it, but it is already selected.
        press(&mut app, "jj ");
        press(&mut app, "fkeep");
        key(&mut app, KeyCode::Enter);
        ctrl_a(&mut app);
        assert_eq!(app.selected, HashSet::from([1, 3]));
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
            app.errors.text().as_deref(),
            Some("Opening #2 failed: no browser")
        );
        app.update(Msg::Opened(1, Ok(())));
        assert_eq!(app.errors.text().unwrap().lines().count(), 1);
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
        for code in [KeyCode::Esc, KeyCode::Char('j'), KeyCode::Enter] {
            key(&mut app, code);
        }
        assert!(matches!(app.mode, Mode::Running(_)));
        assert!(!app.should_quit);
    }

    #[test]
    fn q_cancels_the_approvals_that_have_not_started() {
        let mut app = app_with_rows(6);
        start_approving(&mut app, 6);
        assert_eq!(app.approvals_to_run(), [1, 2, 3, 4]);
        approve(&mut app, 1, Ok(()));

        key(&mut app, KeyCode::Char('q'));
        let Mode::Results(summary) = &app.mode else {
            panic!("the results aren't shown");
        };
        assert!(!app.should_quit);
        assert_eq!((summary.approved, summary.total), (1, 6));
        // 2 to 4 were running; 5 and 6 never started.
        assert_eq!(summary.interrupted, [2, 3, 4]);
        assert_eq!(summary.cancelled, 2);
        assert!(summary.failures.is_empty());
        // Nothing more starts, and the batch's rows are refreshed.
        assert!(app.approvals_to_run().is_empty());
        assert_eq!(app.to_refresh, [1, 2, 3, 4, 5, 6]);
        assert!(app.selected.is_empty());
        // A result that arrives after the cancel changes nothing.
        approve(&mut app, 2, Ok(()));
        assert!(matches!(app.mode, Mode::Results(_)));
    }

    #[test]
    fn q_while_approving_to_merge_cancels_the_merges_too() {
        let mut app = start_approving_and_merging();
        app.approvals_to_run();
        approve(&mut app, 1, Ok(()));
        approve(&mut app, 2, Ok(()));

        key(&mut app, KeyCode::Char('q'));
        assert_eq!(app.merge_to_run(), None);
        let Mode::Results(summary) = &app.mode else {
            panic!("the results aren't shown");
        };
        assert_eq!(summary.approved, 2);
        assert_eq!(summary.interrupted, [3, 4]);
        // PRs 1 and 2 were approved but their merges didn't start.
        assert_eq!(summary.cancelled, 2);
        assert_eq!(summary.merged, Some((0, 0)));
    }

    #[test]
    fn q_while_merging_cancels_the_merges_not_yet_started() {
        let mut app = start_approving_and_merging();
        app.approvals_to_run();
        for number in 1..=4 {
            approve(&mut app, number, Ok(()));
        }
        assert_eq!(app.merge_to_run(), Some((1, false)));
        merge(&mut app, 1, Ok(()));
        assert_eq!(app.merge_to_run(), Some((2, true)));

        key(&mut app, KeyCode::Char('q'));
        assert_eq!(app.merge_to_run(), None);
        let Mode::Results(summary) = &app.mode else {
            panic!("the results aren't shown");
        };
        assert_eq!(summary.approved, 4);
        assert_eq!(summary.merged, Some((1, 2)));
        assert_eq!(summary.interrupted, [2]);
        assert_eq!(summary.cancelled, 2);
        assert_eq!(app.refreshes_to_run(), [1, 2, 3, 4]);
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
            "updatedAt": "2026-01-01T00:00:00Z",
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
            app.errors.text().as_deref(),
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
        app.errors.search = Some("Search failed: bad query".to_string());
        let text = app.errors.text().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "Search failed: bad query");
        // Each failure takes one line, however many lines the error has.
        assert_eq!(lines[1], "Refreshing #1 failed: a b");
        assert_eq!(lines[4], "… and 2 more refreshes failed");

        // New results make them all stale.
        let (id, _) = {
            app.list.query_to_fetch = Some(String::new());
            app.list_to_fetch().unwrap()
        };
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
        assert_eq!(app.errors.text(), None);
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
        assert_eq!(
            app.errors.search.as_deref(),
            Some("Search failed: bad query")
        );
        assert!(!app.list.searching);
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
        assert_eq!(app.scroll.checks, 1);

        // In the comments pane the arrows pick the comment, and a page scrolls its preview.
        key(&mut app, KeyCode::Tab);
        assert!(app.focus == Pane::Comments);
        press(&mut app, "jj");
        assert_eq!(app.scroll.comment, 2);
        key(&mut app, KeyCode::PageDown);
        assert_eq!((app.scroll.comment, app.scroll.comment_scroll), (2, 5));
        key(&mut app, KeyCode::PageUp);
        key(&mut app, KeyCode::PageUp);
        assert_eq!(app.scroll.comment_scroll, 0);
        press(&mut app, "jk");
        assert_eq!(app.scroll.comment, 2);
        key(&mut app, KeyCode::PageDown);
        press(&mut app, "k");
        assert_eq!((app.scroll.comment, app.scroll.comment_scroll), (1, 0));

        key(&mut app, KeyCode::Tab);
        assert!(app.focus == Pane::Description);
        press(&mut app, "jj");
        key(&mut app, KeyCode::PageDown);
        assert_eq!(app.scroll.description, 7);
        key(&mut app, KeyCode::PageUp);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.scroll.description, 1);
        // The highlight stayed where it was, and the other panes didn't scroll.
        assert_eq!(app.table.selected(), Some(2));
        assert_eq!((app.scroll.checks, app.scroll.diff), (1, 0));
        key(&mut app, KeyCode::PageUp);
        assert_eq!(app.scroll.description, 0);

        key(&mut app, KeyCode::Tab);
        assert!(app.focus == Pane::Diff);
        press(&mut app, "k");
        key(&mut app, KeyCode::Down);
        assert_eq!(app.scroll.diff, 1);

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
            (app.scroll.checks, app.scroll.description, app.scroll.diff),
            (0, 0, 0)
        );
        assert_eq!((app.scroll.comment, app.scroll.comment_scroll), (0, 0));

        // A highlight that can't move, at the last row, leaves the scroll alone.
        key(&mut app, KeyCode::Down);
        focus(&mut app, Pane::Description);
        press(&mut app, "j");
        focus(&mut app, Pane::Table);
        key(&mut app, KeyCode::Down);
        assert_eq!(app.scroll.description, 1);
    }

    #[test]
    fn title_shows_repo_and_dry_run() {
        assert_eq!(title(None, false), "PRs");
        assert_eq!(title(Some("o/r"), false), "PRs (o/r)");
        assert_eq!(title(Some("o/r"), true), "PRs (o/r) [DRY RUN]");
    }

    fn limited_app(limit: u32) -> App {
        App::new("PRs".to_string(), "is:pr".to_string(), false).with_limit(limit)
    }

    #[test]
    fn a_list_longer_than_the_limit_is_cut_and_the_title_says_so() {
        let mut app = limited_app(2);
        let (id, _) = app.list_to_fetch().unwrap();
        // GitHub was asked for 3, one more than the limit, and returned all of them.
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=3))));
        assert_eq!(numbers(&app), [1, 2]);
        assert_eq!(app.title(), "PRs - showing first 2, more exist");
    }

    #[test]
    fn a_list_exactly_the_limit_has_no_note() {
        let mut app = limited_app(2);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
        assert_eq!(numbers(&app), [1, 2]);
        assert_eq!(app.title(), "PRs");
    }

    #[test]
    fn the_note_follows_the_latest_list_and_survives_a_failed_search() {
        let mut app = limited_app(1);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
        assert!(app.title().ends_with("more exist"));

        press(&mut app, "/");
        key(&mut app, KeyCode::Enter);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Err(color_eyre::eyre::eyre!("boom"))));
        assert!(app.title().ends_with("more exist"));

        press(&mut app, "/");
        key(&mut app, KeyCode::Enter);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=1))));
        assert_eq!(app.title(), "PRs");
    }

    fn ctrl_r(app: &mut App) {
        app.update(Msg::Key(KeyEvent::new(
            KeyCode::Char('r'),
            KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn ctrl_r_fetches_the_submitted_query() {
        let mut app = app_with_rows(3);
        ctrl_r(&mut app);
        let (_, query) = app.list_to_fetch().unwrap();
        assert_eq!(query, "is:pr");
    }

    #[test]
    fn ctrl_r_is_ignored_while_a_search_runs_and_under_dialogs() {
        let mut app = app_with_rows(3);
        ctrl_r(&mut app);
        assert!(app.list_to_fetch().is_some());
        // The fetch is running.
        ctrl_r(&mut app);
        assert_eq!(app.list_to_fetch(), None);

        let mut app = app_with_rows(3);
        press(&mut app, "?");
        ctrl_r(&mut app);
        assert_eq!(app.list_to_fetch(), None);
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Confirm(_)));
        ctrl_r(&mut app);
        assert_eq!(app.list_to_fetch(), None);
    }

    #[test]
    fn ctrl_r_keeps_the_selection_by_number_when_rows_move() {
        let mut app = app_with_rows(3);
        press(&mut app, " jj ");
        assert_eq!(app.selected, HashSet::from([1, 3]));
        ctrl_r(&mut app);
        let (id, _) = app.list_to_fetch().unwrap();
        // 3 moves to the top, 1 is gone and 4 is new.
        app.update(Msg::PrsLoaded(id, Ok(prs([3, 2, 4]))));
        assert_eq!(numbers(&app), [3, 2, 4]);
        assert_eq!(app.selected, HashSet::from([3]));
        assert_eq!(app.hidden_selected(), 0);
    }

    #[test]
    fn a_new_search_still_clears_the_selection() {
        let mut app = app_with_rows(3);
        press(&mut app, " ");
        press(&mut app, "/");
        key(&mut app, KeyCode::Enter);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs([1, 2, 3]))));
        assert!(app.selected.is_empty());
    }

    #[test]
    fn ctrl_r_keeps_the_highlight_on_the_same_pr_and_clears_the_filter() {
        let mut app = app_with_titles();
        press(&mut app, "jfbeta");
        key(&mut app, KeyCode::Enter);
        ctrl_r(&mut app);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(
            id,
            Ok(vec![titled(3, "c"), titled(2, "Bump beta"), titled(1, "a")]),
        ));
        assert_eq!(app.filter.value(), "");
        assert_eq!(visible_numbers(&app), [3, 2, 1]);
        assert_eq!(app.highlighted().unwrap().pr.number, 2);

        // If it is gone, the first row is highlighted.
        ctrl_r(&mut app);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(vec![titled(8, "x"), titled(9, "y")])));
        assert_eq!(app.highlighted().unwrap().pr.number, 8);
    }

    #[test]
    fn ctrl_r_drops_cached_details_and_diffs_but_a_new_search_keeps_them() {
        let mut app = app_with_rows(2);
        app.details_to_fetch();
        app.diffs_to_fetch();
        assert_eq!(app.details.len(), 2);

        press(&mut app, "/");
        key(&mut app, KeyCode::Enter);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
        assert_eq!((app.details.len(), app.diffs.len()), (2, 2));

        ctrl_r(&mut app);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
        assert_eq!((app.details.len(), app.diffs.len()), (0, 0));
        // So they are fetched again.
        assert_eq!(app.details_to_fetch(), [1, 2]);
    }

    #[test]
    fn a_failed_ctrl_r_keeps_the_list_and_the_selection() {
        let mut app = app_with_rows(3);
        press(&mut app, " ");
        ctrl_r(&mut app);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Err(color_eyre::eyre::eyre!("boom"))));
        assert_eq!(numbers(&app), [1, 2, 3]);
        assert_eq!(app.selected, HashSet::from([1]));
        assert!(app.errors.text().unwrap().contains("boom"));
    }

    fn status(number: u64, state: &str, passing: bool, review: &str, updated: &str) -> PrStatus {
        let rollup = if passing {
            serde_json::json!([{
                "__typename": "CheckRun", "name": "build", "workflowName": "CI",
                "status": "COMPLETED", "conclusion": "SUCCESS", "startedAt": null,
            }])
        } else {
            serde_json::json!([])
        };
        serde_json::from_value(serde_json::json!({
            "number": number,
            "state": state,
            "statusCheckRollup": rollup,
            "mergeable": "MERGEABLE",
            "reviewDecision": review,
            "updatedAt": updated,
        }))
        .unwrap()
    }

    fn update(statuses: Vec<PrStatus>) -> StatusUpdate {
        StatusUpdate {
            statuses,
            failure: None,
        }
    }

    const UNCHANGED: &str = "2026-01-01T00:00:00Z";

    /// Starts a timed refresh, as the timer and the loop would.
    fn start_status_refresh(app: &mut App) -> Option<StatusFetch> {
        app.update(Msg::RefreshDue);
        app.status_to_fetch()
    }

    #[test]
    fn a_timed_refresh_covers_the_open_rows_once_at_a_time() {
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(
            id,
            Ok(vec![pr(1, "OPEN"), pr(2, "MERGED"), pr(3, "OPEN")]),
        ));
        let fetch = start_status_refresh(&mut app).unwrap();
        assert_eq!(fetch.query, "is:pr");
        assert_eq!(fetch.candidates, [1, 3]);
        // One at a time.
        assert!(start_status_refresh(&mut app).is_none());
        app.update(Msg::StatusRefreshed(fetch.id, update(vec![])));
        assert!(start_status_refresh(&mut app).is_some());
    }

    #[test]
    fn a_timed_refresh_waits_for_a_search_a_batch_and_row_refreshes() {
        let mut app = app_with_rows(2);
        press(&mut app, "/");
        key(&mut app, KeyCode::Enter);
        app.list_to_fetch().unwrap();
        assert!(app.list.searching);
        assert!(start_status_refresh(&mut app).is_none());

        let mut app = app_with_rows(2);
        start_approving(&mut app, 1);
        assert!(start_status_refresh(&mut app).is_none());

        let mut app = app_with_rows(2);
        app.to_refresh.push(1);
        assert!(start_status_refresh(&mut app).is_none());

        // Nothing to refresh before a list loads, or when no PR is open.
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        assert!(start_status_refresh(&mut app).is_none());
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(vec![pr(1, "MERGED")])));
        assert!(start_status_refresh(&mut app).is_none());
        assert!(start_status_refresh(&mut app).is_none());
    }

    #[test]
    fn a_timed_refresh_updates_rows_in_place() {
        let mut app = app_with_titles();
        press(&mut app, "jf");
        press(&mut app, "no checks");
        key(&mut app, KeyCode::Enter);
        press(&mut app, " ");
        let fetch = start_status_refresh(&mut app).unwrap();
        let update = update(vec![
            status(2, "OPEN", true, "APPROVED", UNCHANGED),
            status(3, "OPEN", false, "", UNCHANGED),
        ]);
        app.update(Msg::StatusRefreshed(fetch.id, update));

        // Same order, same selection, same highlight, and the filter isn't applied again: the
        // row that now has passing checks stays even though "no checks" no longer matches it.
        assert_eq!(numbers(&app), [1, 2, 3]);
        assert_eq!(app.selected, HashSet::from([2]));
        assert_eq!(visible_numbers(&app), [1, 2, 3]);
        assert_eq!(app.highlighted().unwrap().pr.number, 2);
        let row = &app.rows[1];
        assert_eq!(row.checks.state, CheckState::Passing);
        assert_eq!(row.pr.review_decision, ReviewDecision::Approved);
        assert_eq!(row.pr.title, "Bump beta");
    }

    #[test]
    fn a_pr_that_changed_has_its_details_and_diff_fetched_again() {
        let mut app = app_with_rows(2);
        app.details_to_fetch();
        app.diffs_to_fetch();
        let fetch = start_status_refresh(&mut app).unwrap();
        let update = update(vec![
            status(1, "OPEN", false, "", "2026-02-02T00:00:00Z"),
            status(2, "OPEN", true, "", UNCHANGED),
        ]);
        app.update(Msg::StatusRefreshed(fetch.id, update));
        assert!(!app.details.contains_key(&1) && !app.diffs.contains_key(&1));
        assert!(app.details.contains_key(&2) && app.diffs.contains_key(&2));
        assert_eq!(app.details_to_fetch(), [1]);
    }

    #[test]
    fn a_row_known_to_be_closed_is_not_updated() {
        let mut app = App::new(String::new(), "is:pr".to_string(), false);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(vec![pr(1, "OPEN"), pr(2, "MERGED")])));
        let fetch = start_status_refresh(&mut app).unwrap();
        let update = update(vec![status(
            2,
            "OPEN",
            true,
            "APPROVED",
            "2026-02-02T00:00:00Z",
        )]);
        app.update(Msg::StatusRefreshed(fetch.id, update));
        assert_eq!(app.rows[1].pr.state, PrState::Merged);
        assert_eq!(app.rows[1].pr.review_decision, ReviewDecision::Other);
    }

    #[test]
    fn an_open_row_found_merged_becomes_merged() {
        let mut app = app_with_rows(2);
        let fetch = start_status_refresh(&mut app).unwrap();
        let update = update(vec![status(1, "MERGED", true, "APPROVED", UNCHANGED)]);
        app.update(Msg::StatusRefreshed(fetch.id, update));
        assert_eq!(app.rows[0].pr.state, PrState::Merged);
        assert_eq!(app.rows[0].checks_label(), "· merged");
        // It stays where it was, and is no longer a candidate.
        assert_eq!(numbers(&app), [1, 2]);
        let fetch = start_status_refresh(&mut app).unwrap();
        assert_eq!(fetch.candidates, [2]);
    }

    #[test]
    fn results_for_an_older_list_are_dropped() {
        let mut app = app_with_rows(2);
        let fetch = start_status_refresh(&mut app).unwrap();
        // Ctrl-R replaces the list while the refresh runs.
        ctrl_r(&mut app);
        let (id, _) = app.list_to_fetch().unwrap();
        app.update(Msg::PrsLoaded(id, Ok(prs(1..=2))));
        let update = update(vec![status(1, "MERGED", true, "APPROVED", UNCHANGED)]);
        app.update(Msg::StatusRefreshed(fetch.id, update));
        assert_eq!(app.rows[0].pr.state, PrState::Open);
        // The slot is free again.
        assert!(start_status_refresh(&mut app).is_some());
    }

    #[test]
    fn results_that_began_before_a_batch_are_dropped() {
        let mut app = app_with_rows(2);
        let fetch = start_status_refresh(&mut app).unwrap();
        start_approving(&mut app, 1);
        approve(&mut app, 1, Ok(()));
        assert!(matches!(app.mode, Mode::Results(_)));
        let update = update(vec![status(1, "OPEN", false, "", "2026-02-02T00:00:00Z")]);
        app.update(Msg::StatusRefreshed(fetch.id, update));
        // The old state didn't go over the row the batch changed.
        assert_eq!(app.rows[0].pr.updated_at, UNCHANGED);
    }

    #[test]
    fn a_failed_refresh_is_shown_until_one_succeeds_and_keeps_what_it_got() {
        let mut app = app_with_rows(2);
        let fetch = start_status_refresh(&mut app).unwrap();
        let failed = StatusUpdate {
            statuses: vec![status(2, "OPEN", true, "", UNCHANGED)],
            failure: Some("boom\nsecond line".to_string()),
        };
        app.update(Msg::StatusRefreshed(fetch.id, failed));
        assert_eq!(app.status.failure.as_deref(), Some("boom second line"));
        assert_eq!(app.rows[1].checks.state, CheckState::Passing);
        // It isn't an error above the status bar.
        assert_eq!(app.errors.text(), None);

        let fetch = start_status_refresh(&mut app).unwrap();
        app.update(Msg::StatusRefreshed(fetch.id, update(vec![])));
        assert_eq!(app.status.failure, None);
    }
}
