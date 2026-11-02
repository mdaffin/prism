use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use ansi_to_tui::IntoText;
use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::widgets::TableState;
use regex::Regex;
use tui_input::Input;

use crate::batch::{
    Batch, BatchItem, Choice, Confirm, ConfirmPr, ItemStatus, Summary, auto_merge, finished_status,
};
use crate::gh::{Pr, PrDetails, PrState, PrStatus, StatusUpdate};
use crate::model::{self, CheckState, Checks, one_line};

mod input;
mod panes;
mod work;

pub use panes::{ChecksLayout, CommentLayout, Drawn, Pane, Scroll, to_u16};
use work::forget_failed;
pub use work::{Details, Diff, Fetch};

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
    /// The query to restore when an edit is cancelled.
    submitted_query: String,
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
    /// The local filter box.
    pub filter: Input,
    /// The highlighted row, as a position in `visible`.
    pub table: TableState,
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
}

impl Row {
    pub fn new(pr: Pr) -> Self {
        let checks = model::classify_checks(pr.mergeable, &pr.status_check_rollup);
        Self { pr, checks }
    }

    /// Swaps in a freshly fetched PR, and works out its checks again.
    fn replace(&mut self, pr: Pr) {
        *self = Self::new(pr);
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
    DetailsLoaded(u64, Result<PrDetails>),
    DiffLoaded(u64, Result<String>),
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
            typing: Typing::Nothing,
            list: ListFetch {
                query_to_fetch: Some(query),
                ..ListFetch::default()
            },
            status: StatusRefresh::default(),
            batch_epoch: 0,
            rows: Vec::new(),
            visible: Vec::new(),
            filter: Input::default(),
            table: TableState::default(),
            focus: Pane::Table,
            scroll: Scroll::default(),
            drawn: Drawn::default(),
            errors: Errors::default(),
            prs_to_open: Vec::new(),
            pages_to_open: Vec::new(),
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
            Msg::RefreshDue => self.status.requested = self.can_refresh(),
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
            Msg::DetailsLoaded(number, Err(e)) => {
                self.details.insert(number, Fetch::Failed(e.to_string()));
            }
            Msg::DetailsLoaded(number, Ok(details)) => {
                let body = model::render_markdown(&model::clean_body(details.body.as_deref()));
                let comment_bodies = details
                    .comments
                    .iter()
                    .map(|comment| {
                        model::render_markdown(&model::clean_body(comment.body.as_deref()))
                    })
                    .collect();
                self.details.insert(
                    number,
                    Fetch::Loaded(Details {
                        details,
                        body,
                        comment_bodies,
                    }),
                );
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
        let mut rows: Vec<Row> = prs.into_iter().map(Row::new).collect();
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

    /// `o`: opens what the focused pane is showing. A check or a comment opens its own page; in
    /// the other panes it is Ctrl-O. A missing page is reported, except that a comment without
    /// one falls back to its PR.
    fn open_focused(&mut self) {
        let page = match self.focus {
            Pane::Checks => Some(
                self.highlighted_check_url()
                    .ok_or("The highlighted check has no page"),
            ),
            Pane::Comments => self.highlighted_comment_url().map(Ok),
            _ => None,
        };
        match page {
            Some(Ok(url)) => {
                self.errors.opens.clear();
                self.pages_to_open.push(url);
            }
            Some(Err(message)) => {
                self.errors.opens.clear();
                self.errors.opens.push(message.to_string());
            }
            None => self.open_in_browser(),
        }
    }

    /// The page of the highlighted check of the highlighted PR.
    fn highlighted_check_url(&self) -> Option<String> {
        let results = self.highlighted()?.checks.worst_first();
        let last = results.len().checked_sub(1)?;
        results[self.scroll.check.min(last)].url.clone()
    }

    /// The page of the highlighted comment of the highlighted PR, once its details have loaded.
    fn highlighted_comment_url(&self) -> Option<String> {
        let Some(Fetch::Loaded(loaded)) = self.details.get(&self.highlighted()?.pr.number) else {
            return None;
        };
        let comments = &loaded.details.comments;
        let last = comments.len().checked_sub(1)?;
        comments[self.scroll.comment.min(last)].url.clone()
    }

    /// Ctrl-O: opens the selected PRs, from any pane. With none selected it does nothing.
    fn open_selected(&mut self) {
        if !self.selected.is_empty() {
            self.open_in_browser();
        }
    }

    /// Ctrl-G: reviews the selected PRs, from any pane. With none selected it does nothing.
    fn review_selected(&mut self) {
        if !self.selected.is_empty() {
            self.open_confirm();
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
            self.prs_to_open = numbers;
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
