use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use ansi_to_tui::IntoText;
use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Position;
use ratatui::widgets::TableState;
use regex::Regex;
use tui_input::Input;

use crate::batch::{
    Batch, BatchItem, Choice, Confirm, ConfirmPr, ItemStatus, Summary, auto_merge, finished_status,
};
use crate::gh::{Pr, PrDetails, PrState, PrStatus, ReviewDecision, StatusUpdate};
use crate::history::History;
use crate::model::{self, CheckState, Checks, Sort, one_line};

mod completion;
mod input;
mod panes;
mod work;

pub use panes::{ChecksLayout, CommentLayout, Drawn, Pane, Scroll, to_u16};
pub use work::{Details, Diff, Fetch, FetchId, RepoNames};
use work::{forget_failed, is_current, names_loaded};

/// The most failed row refreshes, and the most failed opens, to list above the status bar.
const MAX_LISTED_ERRORS: usize = 3;

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
    /// The highlight in the list of suggestions under the search box.
    completion_view: completion::CompletionView,
    /// The query to restore when an edit is cancelled.
    submitted_query: String,
    /// The queries submitted before, walked with Up and Down in the search box.
    history: History,
    /// Which box, if any, is taking the keys.
    pub typing: Typing,
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
    /// How the list is ordered, and whether that is the other way round.
    sort: Sort,
    descending: bool,
    /// The local filter box.
    pub filter: Input,
    /// The box that edits the column regex. It starts from the regex in use each time it opens.
    pub columns_input: Input,
    /// Why the regex in `columns_input` was refused, until it is edited again.
    pub columns_error: Option<String>,
    /// The highlighted row, as a position in `visible`.
    pub table: TableState,
    /// Columns the table is scrolled sideways, past its first columns. The view clamps it. Unlike
    /// `scroll` it stays when the highlight moves to another PR.
    pub list_x: u16,
    /// The pane that Up/Down and PgUp/PgDn act on.
    pub focus: Pane,
    /// How far the panes are scrolled, and the comment picked.
    pub scroll: Scroll,
    /// What the view found out as it drew: set by it, read by `update` and the mouse.
    pub drawn: Drawn,
    /// The numbers of the PRs marked with Space.
    pub selected: HashSet<u64>,
    /// What to show above the status bar.
    pub errors: Errors,
    /// PRs to open in the browser, in order.
    prs_to_open: Vec<u64>,
    /// Pages (a check's or a comment's) to open in the browser, in order.
    pages_to_open: Vec<String>,
    /// Text for the clipboard.
    text_to_copy: Option<String>,
    /// What the last key did that the screen doesn't show, such as a copy. The next key clears it.
    pub note: Option<String>,
    /// Details by PR number. An entry is added when its fetch starts, so a PR is fetched once.
    pub details: HashMap<u64, Fetch<Details>>,
    /// Diffs by PR number. An entry is added when its fetch starts, so a PR is fetched once.
    pub diffs: HashMap<u64, Fetch<Diff>>,
    /// The labels and branches for completing the search query.
    pub names: RepoNames,
    /// The id of the latest details, diff or names fetch started: see [`FetchId`].
    next_fetch_id: FetchId,
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
    /// Whether the terminal has lost focus, which many report. Nobody is looking, so it pauses.
    unfocused: bool,
    /// Whether `p` paused it, for terminals that don't report focus.
    paused_by_hand: bool,
    /// Whether a refresh came due while it was paused, to run when it resumes.
    missed: bool,
}

impl StatusRefresh {
    /// Whether the timed refresh is paused. Ctrl-R still runs the query again.
    pub fn paused(&self) -> bool {
        self.unfocused || self.paused_by_hand
    }
}

/// The errors shown above the status bar.
#[derive(Default)]
pub struct Errors {
    /// Why the last search failed.
    search: Option<String>,
    /// One line for each row refresh that failed, shown after `search`.
    refreshes: Vec<String>,
    /// One line for each PR or page that failed to open in the browser, shown after `refreshes`.
    opens: Vec<String>,
}

impl Errors {
    /// Why the search failed, then the row refreshes and opens that failed. Only the first few
    /// of those are listed, as the space for them is small.
    pub fn text(&self) -> Option<String> {
        let mut lines: Vec<String> = self.search.iter().cloned().collect();
        push_listed(&mut lines, &self.refreshes, "refreshes failed");
        push_listed(&mut lines, &self.opens, "failed to open");
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

/// The box that is taking the keys, if any.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    Nothing,
    /// The search box.
    Search,
    /// The filter box.
    Filter,
    /// The box that edits the column regex.
    Columns,
}

pub(super) use crate::gh::failure_text;

/// What `o` opens for the highlighted item of a pane.
pub enum Page {
    Url(String),
    /// The item has no page of its own.
    Missing,
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

impl ConfirmOpen {
    /// Moves between Open and Cancel. Returns `Some(true)` to open the PRs and `Some(false)` to
    /// close the dialog without opening them.
    fn on_key(&mut self, key: KeyEvent) -> Option<bool> {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => self.open = true,
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => self.open = false,
            KeyCode::Enter => return Some(self.open),
            KeyCode::Esc | KeyCode::Char('q') => return Some(false),
            _ => {}
        }
        None
    }
}

pub struct Row {
    pub pr: Pr,
    pub checks: Checks,
    /// Where GitHub listed it, so that PRs that tie in a sort stay in that order however often the
    /// list is sorted.
    listed_at: usize,
}

impl Row {
    pub fn new(pr: Pr) -> Self {
        let checks = model::classify_checks(pr.mergeable, &pr.status_check_rollup);
        Self {
            pr,
            checks,
            listed_at: 0,
        }
    }

    /// Swaps in a freshly fetched PR, and works out its checks again. It keeps its place in the
    /// order GitHub listed.
    fn replace(&mut self, pr: Pr) {
        let listed_at = self.listed_at;
        *self = Self::new(pr);
        self.listed_at = listed_at;
    }

    /// How this row sorts against another: open PRs before the rest, then by `sort` (the other
    /// way round if `descending`), then in the order GitHub listed them. The title sorts by the
    /// cells of the table's title columns, so by what is shown.
    fn order(
        &self,
        other: &Row,
        sort: Sort,
        descending: bool,
        columns: Option<&Regex>,
    ) -> std::cmp::Ordering {
        let closed = |row: &Row| row.pr.state != PrState::Open;
        // The review column's order: those that need one, then changes requested, then approved.
        let review = |row: &Row| match row.pr.review_decision {
            ReviewDecision::Other => 0,
            ReviewDecision::ChangesRequested => 1,
            ReviewDecision::Approved => 2,
        };
        let cells = |row: &Row| -> Vec<String> {
            let cells = row.title_cells(columns);
            cells.iter().map(|cell| cell.to_lowercase()).collect()
        };
        closed(self)
            .cmp(&closed(other))
            .then_with(|| {
                let by_sort = match sort {
                    Sort::Checks => self.checks.state.rank().cmp(&other.checks.state.rank()),
                    Sort::Number => self.pr.number.cmp(&other.pr.number),
                    Sort::Title => cells(self).cmp(&cells(other)),
                    Sort::Review => review(self).cmp(&review(other)),
                    Sort::Author => self
                        .pr
                        .author
                        .login
                        .to_lowercase()
                        .cmp(&other.pr.author.login.to_lowercase()),
                    // Timestamps in this form sort as text.
                    Sort::Updated => self.pr.updated_at.cmp(&other.pr.updated_at),
                };
                if descending {
                    by_sort.reverse()
                } else {
                    by_sort
                }
            })
            .then_with(|| self.listed_at.cmp(&other.listed_at))
    }

    /// Takes what a timed refresh brings, and works out the checks again. The title stays.
    fn apply(&mut self, status: PrStatus) {
        let checks = model::classify_checks(status.mergeable, &status.status_check_rollup);
        self.pr.update(status);
        self.checks = checks;
    }

    /// The checks column as drawn: the checks of a closed or merged PR no longer matter.
    pub fn checks_label(&self) -> &'static str {
        match self.pr.state {
            PrState::Open => self.checks.state.label(),
            PrState::Closed => "· closed",
            PrState::Merged => "· merged",
        }
    }

    /// Whether the filter matches in some column for each of its terms. The author counts even
    /// when its column is hidden, which is only when every row has the same one.
    fn matches(&self, filter: &mut model::Filter, title_columns: Option<&Regex>) -> bool {
        let cells: Vec<String> = [
            format!("#{}", self.pr.number),
            self.checks_label().to_string(),
            model::review_label(self.pr.review_decision).to_string(),
            self.pr.author.login.clone(),
        ]
        .into_iter()
        .chain(self.title_cells(title_columns))
        .collect();
        filter.matches(&cells)
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

/// What a timed status refresh was started for. Its results are dropped if either has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshId {
    /// The id of the list fetch it followed.
    list_id: u64,
    /// The batch state: see [`App::batch_epoch`].
    batch_epoch: u64,
}

pub enum Msg {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// The result of the list fetch with this id.
    PrsLoaded(u64, Result<Vec<Pr>>),
    /// The result of the details fetch with this id for this PR, dropped if the PR's entry has
    /// been dropped or fetched again since (see [`FetchId`]).
    DetailsLoaded(u64, FetchId, Result<PrDetails>),
    /// Likewise for a diff.
    DiffLoaded(u64, FetchId, Result<String>),
    /// The repository's labels, branches, users or teams, from the fetch with this id, dropped if
    /// it is not the latest.
    LabelsLoaded(FetchId, Result<Vec<String>>),
    BranchesLoaded(FetchId, Result<Vec<String>>),
    UsersLoaded(FetchId, Result<Vec<String>>),
    TeamsLoaded(FetchId, Result<Vec<String>>),
    /// The result of approving this PR, in the batch with this epoch (see
    /// [`App::batch_epoch`]). One from an earlier batch, such as a cancelled one, is dropped.
    Approved(u64, u64, Result<()>),
    /// The result of merging this PR, or of turning on auto-merge, in the batch with this epoch.
    Merged(u64, u64, Result<()>),
    /// The result of fetching this PR again to refresh its row.
    PrRefreshed(u64, Result<Pr>),
    /// The result of opening this PR in the browser.
    Opened(u64, Result<()>),
    /// The result of opening a page that isn't a PR's, in the browser.
    PageOpened(Result<()>),
    /// Time to advance the spinner.
    Tick,
    /// Time for a timed status refresh.
    RefreshDue,
    /// The terminal gained or lost the focus.
    FocusGained,
    FocusLost,
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
            columns_input: Input::default(),
            columns_error: None,
            dry_run,
            mode: Mode::Loading,
            query: Input::new(query.clone()),
            completion_view: completion::CompletionView::default(),
            submitted_query: query.clone(),
            history: History::load(None, crate::history::DEFAULT_LIMIT),
            typing: Typing::Nothing,
            list: ListFetch {
                query_to_fetch: Some(query),
                ..ListFetch::default()
            },
            status: StatusRefresh::default(),
            batch_epoch: 0,
            rows: Vec::new(),
            visible: Vec::new(),
            sort: Sort::default(),
            descending: false,
            filter: Input::default(),
            table: TableState::default(),
            list_x: 0,
            focus: Pane::Table,
            scroll: Scroll::default(),
            drawn: Drawn::default(),
            errors: Errors::default(),
            prs_to_open: Vec::new(),
            pages_to_open: Vec::new(),
            text_to_copy: None,
            note: None,
            selected: HashSet::new(),
            details: HashMap::new(),
            diffs: HashMap::new(),
            names: RepoNames::default(),
            next_fetch_id: 0,
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

    /// Walks and adds to `history` in the search box.
    pub fn with_history(mut self, history: History) -> Self {
        self.history = history;
        self
    }

    /// Writes the search history, for when the app ends.
    pub fn save_history(&self) {
        self.history.save();
    }

    /// Shows a column for each named group of `columns` in place of the title.
    pub fn with_columns(mut self, columns: Option<Regex>) -> Self {
        self.columns = columns;
        self
    }

    /// Opens the columns box on the regex in use, or on nothing.
    pub(super) fn edit_columns(&mut self) {
        let current = self.columns.as_ref().map_or("", Regex::as_str);
        self.columns_input = Input::new(current.to_string());
        self.columns_error = None;
        self.typing = Typing::Columns;
    }

    /// Uses the regex in the columns box, and closes it. Text that isn't a valid column regex
    /// leaves the box open with the reason and the old regex in use; no text means no columns.
    fn apply_columns(&mut self) {
        let text = self.columns_input.value().trim();
        let columns = if text.is_empty() {
            None
        } else {
            match model::parse_column_regex(text) {
                Ok(regex) => Some(regex),
                Err(error) => {
                    self.columns_error = Some(model::regex_error_line(&error).to_string());
                    return;
                }
            }
        };
        self.columns = columns;
        self.typing = Typing::Nothing;
        // The filter matches the columns shown, and a sort by title goes by them too.
        self.resort();
    }

    /// The title, with a note when the list was cut short, and one for a sort that has no column
    /// of the table to mark: the update time, and the author when the column is hidden because
    /// every PR has the same one.
    pub fn title(&self) -> String {
        let mut title = self.title.clone();
        let unmarked = match self.sort {
            Sort::Updated => Some("updated"),
            Sort::Author if !self.shows_author() => Some("author"),
            _ => None,
        };
        if let Some(name) = unmarked {
            let arrow = if self.descending { '▼' } else { '▲' };
            title.push_str(&format!(" · sorted by {name} {arrow}"));
        }
        if self.truncated {
            title.push_str(&format!(" - showing first {}, more exist", self.limit));
        }
        title
    }

    /// How the list is sorted and whether that is the other way round, for marking the column.
    pub fn sort(&self) -> (Sort, bool) {
        (self.sort, self.descending)
    }

    /// `s`: sorts the list by the next of the sorts, which come round again, smallest first. It
    /// stays for the session and applies to every list that loads.
    pub(super) fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        // With one author for every PR there is no author column, so nothing to sort by.
        if self.sort == Sort::Author && !self.shows_author() {
            self.sort = self.sort.next();
        }
        self.descending = false;
        self.resort();
    }

    /// The sort that the header at `position` stands for, if there is one.
    pub(super) fn sort_at(&self, position: Position) -> Option<Sort> {
        let header = &self.drawn.areas.table_header;
        header
            .iter()
            .find(|(area, _)| area.contains(position))
            .map(|&(_, sort)| sort)
    }

    /// A click on a header: sorts by its column, smallest first; clicked again, the other way
    /// round; and again, back to how the list is sorted to begin with.
    pub(super) fn click_sort(&mut self, sort: Sort) {
        if self.sort != sort {
            self.sort = sort;
            self.descending = false;
        } else if !self.descending {
            self.descending = true;
        } else {
            self.sort = Sort::default();
            self.descending = false;
        }
        self.resort();
    }

    /// `S`: sorts the list the other way round.
    pub(super) fn reverse_sort(&mut self) {
        self.descending = !self.descending;
        self.resort();
    }

    /// Puts `rows` in the order of the sort. The highlight stays on its PR, and the selection,
    /// which is by number, is untouched; the filter is applied to the new order.
    fn resort(&mut self) {
        let before = self.highlighted().map(|row| row.pr.number);
        let (sort, descending, columns) = (self.sort, self.descending, self.columns.as_ref());
        self.rows
            .sort_by(|a, b| a.order(b, sort, descending, columns));
        // `visible` is positions in `rows`, which have moved.
        self.visible = (0..self.rows.len()).collect();
        self.refilter_keeping(before);
    }

    /// Whether the spinner needs ticks.
    pub fn animating(&self) -> bool {
        self.list.searching || self.refreshing() || matches!(self.mode, Mode::Running(_))
    }

    /// Whether any rows are being refreshed or are waiting to be.
    pub fn refreshing(&self) -> bool {
        !self.refreshing.is_empty() || !self.to_refresh.is_empty()
    }

    /// What a status refresh started now would be tied to: the list and the batch state it saw.
    fn refresh_id(&self) -> RefreshId {
        RefreshId {
            list_id: self.list.id,
            batch_epoch: self.batch_epoch,
        }
    }

    /// `p`: pauses the timed refresh, or resumes it.
    fn toggle_refresh_paused(&mut self) {
        self.status.paused_by_hand = !self.status.paused_by_hand;
        self.run_missed_refresh();
    }

    /// Asks for the refresh that came due while paused, now that nothing holds it back.
    fn run_missed_refresh(&mut self) {
        if !self.status.paused() && std::mem::take(&mut self.status.missed) {
            self.status.requested = self.can_refresh();
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
            row.apply(status);
        }
        self.status.failure = update.failure.map(|failure| one_line(&failure));
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
        self.refilter_keeping(before);
    }

    /// Works out `visible` from the filter, with the highlight on PR `before` if it is visible,
    /// else on the first row.
    fn refilter_keeping(&mut self, before: Option<u64>) {
        let mut filter = model::Filter::new(self.filter.value());
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.matches(&mut filter, self.columns.as_ref()))
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

    /// Stops typing in the filter box. Typing in the search box carries on.
    fn stop_filtering(&mut self) {
        if self.typing == Typing::Filter {
            self.typing = Typing::Nothing;
        }
    }

    fn clear_filter(&mut self) {
        self.filter = Input::default();
        self.stop_filtering();
        self.refilter();
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::Mouse(mouse) => self.on_mouse(mouse),
            Msg::Tick => self.spinner = self.spinner.wrapping_add(1),
            Msg::RefreshDue if self.status.paused() => self.status.missed = true,
            Msg::RefreshDue => self.status.requested = self.can_refresh(),
            Msg::FocusLost => self.status.unfocused = true,
            Msg::FocusGained => {
                self.status.unfocused = false;
                self.run_missed_refresh();
            }
            Msg::StatusRefreshed(id, update) => {
                self.status.in_flight = false;
                // Results for a list or batch state that has since changed are dropped.
                if id == self.refresh_id() {
                    self.apply_statuses(update);
                }
            }
            Msg::Approved(epoch, number, result) if epoch == self.batch_epoch => {
                self.approved(number, result)
            }
            Msg::Merged(epoch, number, result) if epoch == self.batch_epoch => {
                self.merged(number, result)
            }
            Msg::Approved(..) | Msg::Merged(..) => {}
            Msg::Opened(number, Err(e)) => {
                let message = format!("Opening #{number} failed: {e}");
                self.errors.opens.push(one_line(&message));
            }
            Msg::PageOpened(Err(e)) => {
                let message = format!("Opening the page failed: {e}");
                self.errors.opens.push(one_line(&message));
            }
            Msg::Opened(_, Ok(())) | Msg::PageOpened(Ok(())) => {}
            Msg::PrRefreshed(number, result) => self.row_refreshed(number, result),
            Msg::LabelsLoaded(id, result) => names_loaded(&mut self.names.labels, id, result),
            Msg::BranchesLoaded(id, result) => names_loaded(&mut self.names.branches, id, result),
            Msg::UsersLoaded(id, result) => names_loaded(&mut self.names.users, id, result),
            Msg::TeamsLoaded(id, result) => names_loaded(&mut self.names.teams, id, result),
            Msg::DetailsLoaded(number, id, _) if !is_current(&self.details, number, id) => {}
            Msg::DiffLoaded(number, id, _) if !is_current(&self.diffs, number, id) => {}
            Msg::DetailsLoaded(number, _, Err(e)) => {
                self.details.insert(number, Fetch::Failed(failure_text(&e)));
            }
            Msg::DetailsLoaded(number, _, Ok(details)) => {
                let body = model::render_markdown(&model::clean_body(details.body.as_deref()));
                let comment_bodies = details.comments.iter().map(model::render_comment).collect();
                self.details.insert(
                    number,
                    Fetch::Loaded(Details {
                        details,
                        body,
                        comment_bodies,
                    }),
                );
            }
            Msg::DiffLoaded(number, _, Err(e)) => {
                self.diffs.insert(number, Fetch::Failed(failure_text(&e)));
            }
            Msg::DiffLoaded(number, _, Ok(diff)) => {
                // A tab would be drawn as a single cell. Parsing here rather than when drawing
                // means each diff is parsed once.
                let diff = match diff.replace('\t', "    ").into_text() {
                    Ok(text) => Fetch::Loaded(Diff::new(text)),
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
        let mut rows: Vec<Row> = prs.into_iter().map(Row::new).collect();
        for (i, row) in rows.iter_mut().enumerate() {
            row.listed_at = i;
        }
        // Open PRs first, then as the sort says; closed and merged PRs can't be acted on.
        let (sort, descending) = (self.sort, self.descending);
        rows.sort_by(|a, b| a.order(b, sort, descending, self.columns.as_ref()));
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
        self.stop_filtering();
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
            self.names.forget_failed();
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
                    row.replace(pr);
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

    /// The numbers of the selected PRs, in table order.
    fn selected_numbers(&self) -> Vec<u64> {
        self.rows
            .iter()
            .map(|row| row.pr.number)
            .filter(|number| self.selected.contains(number))
            .collect()
    }

    /// `o` and, in the checks pane, Enter: opens the highlighted item of the focused pane. The
    /// table opens its highlighted PR, a check or a comment its own page, and the description and
    /// the diff nothing. A missing page is reported, except that a comment without one falls back
    /// to its PR.
    fn open_focused(&mut self) {
        if matches!(self.focus, Pane::Approvers | Pane::Description | Pane::Diff) {
            return;
        }
        self.errors.opens.clear();
        match (self.focus, self.highlighted_page()) {
            (_, Some(Page::Url(url))) => self.pages_to_open.push(url),
            (Pane::Checks, _) => self
                .errors
                .opens
                .push("The highlighted check has no page".to_string()),
            _ => {
                let number = self.highlighted().map(|row| row.pr.number);
                self.prs_to_open = number.into_iter().collect();
            }
        }
    }

    /// `y`: copies what `o` would open: the URL of the highlighted item, or the PR's where it has
    /// none of its own. The description and the diff have nothing to copy.
    fn copy_focused(&mut self) {
        let (text, note) = match (self.focus, self.highlighted_page()) {
            (_, Some(Page::Url(url))) => (url, "Copied the URL"),
            (Pane::Checks, _) => {
                self.note = Some("The highlighted check has no page to copy".to_string());
                return;
            }
            (Pane::Table | Pane::Comments, _) => match self.highlighted() {
                Some(row) => (row.pr.url.clone(), "Copied the PR's URL"),
                None => return,
            },
            (Pane::Approvers | Pane::Description | Pane::Diff, _) => return,
        };
        self.text_to_copy = Some(text);
        self.note = Some(note.to_string());
    }

    /// Enter: acts on the highlighted item of the focused pane. The table reviews its highlighted
    /// PR, the comments expand or collapse the highlighted comment, the checks open it, and the
    /// description and the diff do nothing.
    fn enter_focused(&mut self) {
        match self.focus {
            Pane::Table => {
                let numbers = self.highlighted().map(|row| row.pr.number);
                self.open_confirm(numbers.into_iter().collect());
            }
            Pane::Checks => self.open_focused(),
            Pane::Comments => self.toggle_comment(),
            Pane::Approvers | Pane::Description | Pane::Diff => {}
        }
    }

    /// Whether the search box holds text that has not been searched for, after a click away from
    /// it.
    pub fn query_is_unsearched(&self) -> bool {
        self.query.value() != self.submitted_query
    }

    /// Whether the table has an Author column: not when the search names one author, as every row
    /// would show the same name.
    pub fn shows_author(&self) -> bool {
        !crate::query::names_one_author(&self.submitted_query)
    }

    /// The page that `o` opens for the highlighted item of the focused pane, for the status bar.
    /// `None` when the pane has no such item: the table opens the PR itself, and the checks and
    /// comments are empty or still loading.
    pub fn highlighted_page(&self) -> Option<Page> {
        let url = match self.focus {
            Pane::Checks => {
                let results = self.highlighted()?.checks.worst_first();
                let last = results.len().checked_sub(1)?;
                results[self.scroll.check.min(last)].url.clone()
            }
            Pane::Comments => {
                let Some(Fetch::Loaded(loaded)) = self.details.get(&self.highlighted()?.pr.number)
                else {
                    return None;
                };
                let comments = &loaded.details.comments;
                let last = comments.len().checked_sub(1)?;
                comments[self.scroll.comment.min(last)].url.clone()
            }
            Pane::Table | Pane::Approvers | Pane::Description | Pane::Diff => return None,
        };
        Some(url.map_or(Page::Missing, Page::Url))
    }

    /// Ctrl-O: opens the selected PRs, from any pane. With none selected it does nothing.
    fn open_selected(&mut self) {
        let numbers = self.selected_numbers();
        if !numbers.is_empty() {
            self.open_in_browser(numbers);
        }
    }

    /// Ctrl-G: reviews the selected PRs, from any pane. With none selected it does nothing.
    fn review_selected(&mut self) {
        self.open_confirm(self.selected_numbers());
    }

    /// Opens these PRs in the browser. Asks first when there are more than
    /// [`MAX_OPEN_WITHOUT_ASKING`].
    fn open_in_browser(&mut self, numbers: Vec<u64>) {
        self.errors.opens.clear();
        if numbers.len() > MAX_OPEN_WITHOUT_ASKING {
            self.mode = Mode::ConfirmOpen(ConfirmOpen {
                numbers,
                open: true,
            });
        } else {
            self.prs_to_open = numbers;
        }
    }

    /// Opens the confirm dialog for these PRs, in table order.
    fn open_confirm(&mut self, numbers: Vec<u64>) {
        let rows: Vec<&Row> = self
            .rows
            .iter()
            .filter(|row| numbers.contains(&row.pr.number))
            .collect();
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
        if let Some(number) = self.highlighted().map(|row| row.pr.number) {
            self.toggle(number);
        }
    }

    fn toggle(&mut self, number: u64) {
        toggle(&mut self.selected, number);
    }
}

/// Removes `value` from `set` if it is there, else adds it.
fn toggle<T: Eq + Hash>(set: &mut HashSet<T>, value: T) {
    if !set.remove(&value) {
        set.insert(value);
    }
}

#[cfg(test)]
mod tests;
