//! The background work the app asks for: what to fetch or run next, and what was fetched.
//! `update` decides, and the caller in `main` starts it and sends back a `Msg`.

use std::collections::{HashMap, HashSet};

use ansi_to_tui::IntoText;
use ratatui::text::Text;

use super::{App, Mode, Pane, RefreshId, Typing};
use crate::batch::{ItemStatus, auto_merge};
use crate::gh::{PrDetails, PrState};
use crate::model::Markdown;
use crate::query::{self, Source};

/// The most detail fetches to run at once.
const MAX_DETAIL_FETCHES: usize = 4;

/// The most diffs fetched at once.
const MAX_DIFF_FETCHES: usize = 4;

/// The most diffs kept. Diffs are prefetched for the rows nearest the highlight (see
/// [`App::diffs_to_fetch`]), and the others are dropped once there are more than this, so a long
/// list doesn't hold every diff in memory.
pub(super) const MAX_CACHED_DIFFS: usize = 30;

/// The most approvals to run at once.
pub(super) const MAX_APPROVALS: usize = 4;

/// The most row refreshes to run at once.
pub(super) const MAX_REFRESHES: usize = 4;

/// Tells one fetch from another for the same PR. A result that arrives after its entry was
/// dropped, perhaps for a newer fetch, no longer has the id of the entry and is dropped too.
pub type FetchId = u64;

/// Something fetched in the background for a PR, such as its details or its diff.
pub enum Fetch<T> {
    Loading(FetchId),
    Loaded(T),
    /// Kept rather than removed, so a failing PR isn't fetched again on every update.
    Failed(String),
}

/// A PR's details, fetched in the background.
pub struct Details {
    pub details: PrDetails,
    /// The body with HTML comments and extra blank lines removed, rendered as markdown once here
    /// rather than on every draw.
    pub body: Markdown,
    /// Each comment's body, rendered the same way and in the same order as `details.comments`.
    pub comment_bodies: Vec<Markdown>,
}

/// A PR's diff with gh's colours.
pub struct Diff {
    pub text: Text<'static>,
    /// The columns of its widest line, worked out once when it loads, as a diff can be very long
    /// and the view needs it to scroll sideways.
    pub widest: usize,
    /// Its lines as text, for selecting, made once when it loads.
    pub rows: std::sync::Arc<Vec<super::selection::TextRow>>,
}

impl Diff {
    /// Reads gh's coloured output. This is slow for a huge diff, so the caller runs it off the UI
    /// thread, and the result is sent as a message.
    pub fn parse(raw: &str) -> color_eyre::eyre::Result<Self> {
        // A tab would be drawn as a single cell.
        let text = raw
            .replace('\t', "    ")
            .into_text()
            .map_err(|e| color_eyre::eyre::eyre!("couldn't read the colours: {e}"))?;
        Ok(Self::new(text))
    }

    pub fn new(text: Text<'static>) -> Self {
        let widest = text
            .lines
            .iter()
            .map(|line| line.width())
            .max()
            .unwrap_or(0);
        let rows = text
            .lines
            .iter()
            .map(|line| super::selection::TextRow {
                text: line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect(),
                ..Default::default()
            })
            .collect();
        Self {
            text,
            widest,
            rows: std::sync::Arc::new(rows),
        }
    }
}

/// Marks the PRs in `order` that have no entry in `fetches` as loading, and returns them with
/// their fetch ids, as many as there are free slots out of `max`. The caller must start their
/// fetches, and send the ids back with the results.
fn start_fetches<T>(
    fetches: &mut HashMap<u64, Fetch<T>>,
    next_id: &mut FetchId,
    order: Vec<u64>,
    max: usize,
) -> Vec<(u64, FetchId)> {
    let in_flight = fetches
        .values()
        .filter(|fetch| matches!(fetch, Fetch::Loading(_)))
        .count();
    let numbers: Vec<u64> = order
        .into_iter()
        .filter(|number| !fetches.contains_key(number))
        .take(max.saturating_sub(in_flight))
        .collect();
    numbers
        .into_iter()
        .map(|number| {
            *next_id += 1;
            fetches.insert(number, Fetch::Loading(*next_id));
            (number, *next_id)
        })
        .collect()
}

/// Whether a result with this id is the one the entry for `number` is waiting for. If the entry
/// was dropped, such as by a refresh, or dropped and started again, it is not.
pub(super) fn is_current<T>(fetches: &HashMap<u64, Fetch<T>>, number: u64, id: FetchId) -> bool {
    matches!(fetches.get(&number), Some(Fetch::Loading(current)) if *current == id)
}

/// Drops the fetches still running for PRs that are not in `listed`. Nothing shows them any more,
/// and while they stayed they took up slots that the new list's PRs need. Their results, when
/// they arrive, no longer match an entry and are dropped (see [`is_current`]).
pub(super) fn forget_unlisted_loading<T>(
    fetches: &mut HashMap<u64, Fetch<T>>,
    listed: &HashSet<u64>,
) {
    fetches.retain(|number, fetch| listed.contains(number) || !matches!(fetch, Fetch::Loading(_)));
}

/// Drops the failed fetches, so they are tried again.
pub(super) fn forget_failed<T>(fetches: &mut HashMap<u64, Fetch<T>>) {
    fetches.retain(|_, fetch| !matches!(fetch, Fetch::Failed(_)));
}

/// The names the repository has, fetched to complete the search query. The labels when the search
/// box is first opened, and the others when a qualifier that uses them is first typed. `None`
/// until asked for.
#[derive(Default)]
pub struct RepoNames {
    pub(super) labels: Option<Fetch<Vec<String>>>,
    pub(super) branches: Option<Fetch<Vec<String>>>,
    pub(super) users: Option<Fetch<Vec<String>>>,
    pub(super) teams: Option<Fetch<Vec<String>>>,
}

/// The names fetches that [`App::names_to_fetch`] started, with their ids.
#[derive(Default, Debug, PartialEq)]
pub struct NamesToFetch {
    pub labels: Option<FetchId>,
    pub branches: Option<FetchId>,
    pub users: Option<FetchId>,
    pub teams: Option<FetchId>,
}

impl RepoNames {
    /// The labels fetched, or none while they load or if that failed.
    pub fn labels(&self) -> &[String] {
        loaded(&self.labels)
    }

    /// The branches fetched, or none while they load or if that failed.
    pub fn branches(&self) -> &[String] {
        loaded(&self.branches)
    }

    /// The users fetched who can be assigned, or none while they load or if that failed.
    pub fn users(&self) -> &[String] {
        loaded(&self.users)
    }

    /// The owner's teams fetched, or none while they load or if that failed.
    pub fn teams(&self) -> &[String] {
        loaded(&self.teams)
    }

    /// Drops a failed fetch, so that it is tried again the next time the box is opened.
    pub(super) fn forget_failed(&mut self) {
        for fetch in [
            &mut self.labels,
            &mut self.branches,
            &mut self.users,
            &mut self.teams,
        ] {
            if matches!(fetch, Some(Fetch::Failed(_))) {
                *fetch = None;
            }
        }
    }
}

fn loaded(fetch: &Option<Fetch<Vec<String>>>) -> &[String] {
    match fetch {
        Some(Fetch::Loaded(names)) => names,
        _ => &[],
    }
}

/// Takes a result into `fetch` if it is the one it waits for, and drops it otherwise.
pub(super) fn names_loaded(
    fetch: &mut Option<Fetch<Vec<String>>>,
    id: FetchId,
    result: color_eyre::Result<Vec<String>>,
) {
    if !matches!(fetch, Some(Fetch::Loading(current)) if *current == id) {
        return;
    }
    *fetch = Some(match result {
        Ok(names) => Fetch::Loaded(names),
        Err(e) => Fetch::Failed(super::failure_text(&e)),
    });
}

/// A timed status refresh for the caller to start.
pub struct StatusFetch {
    pub id: RefreshId,
    /// The query of the list shown.
    pub query: String,
    /// The open PRs shown, to look up one by one any that the query no longer lists.
    pub candidates: Vec<u64>,
}

impl App {
    /// Rows to refresh now, up to [`MAX_REFRESHES`] at once. The caller must start them.
    pub fn refreshes_to_fetch(&mut self) -> Vec<u64> {
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
        std::mem::take(&mut self.prs_to_open)
    }

    /// Delivers a details result as if its fetch had just started for this PR, which replaces
    /// any entry it had.
    #[cfg(test)]
    pub fn details_loaded(&mut self, number: u64, result: color_eyre::eyre::Result<PrDetails>) {
        self.next_fetch_id += 1;
        let id = self.next_fetch_id;
        self.details.insert(number, Fetch::Loading(id));
        self.update(super::Msg::DetailsLoaded(number, id, result));
    }

    /// Likewise for a diff.
    #[cfg(test)]
    pub fn diff_loaded(&mut self, number: u64, result: color_eyre::eyre::Result<String>) {
        self.next_fetch_id += 1;
        let id = self.next_fetch_id;
        self.diffs.insert(number, Fetch::Loading(id));
        let result = result.and_then(|raw| Diff::parse(&raw));
        self.update(super::Msg::DiffLoaded(number, id, result));
    }

    /// The text to put on the clipboard now. The caller must send it to the terminal.
    pub fn copy_to_run(&mut self) -> Option<String> {
        self.text_to_copy.take()
    }

    /// The pages to open in the browser now. The caller must open them.
    pub fn pages_to_run(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pages_to_open)
    }

    /// The epoch of the running batch, which the results of its actions carry back.
    pub fn batch_epoch(&self) -> u64 {
        self.batch_epoch
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

    /// The next merge to start, and whether it should use auto-merge. A PR can be merged as soon
    /// as it is approved, while others are still being approved, but merges run one at a time, so
    /// this returns nothing while one is working. Marks it as working, so the caller must start it.
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

    /// The names to fetch now, if they haven't been asked for. The labels when the search box is
    /// open, and the branches, users or teams only once a value that uses them is being typed, as
    /// they cost more and are rarely wanted. Marks them as loading, so the caller must start the
    /// fetches.
    pub fn names_to_fetch(&mut self) -> NamesToFetch {
        let mut started = NamesToFetch::default();
        if self.typing != Typing::Search {
            return started;
        }
        let source = query::source_at(self.query.value(), self.query.cursor());
        let wanted = [
            true,
            source == Some(Source::Branches),
            source == Some(Source::Users),
            source == Some(Source::Teams),
        ];
        for ((fetch, id), wanted) in [
            (&mut self.names.labels, &mut started.labels),
            (&mut self.names.branches, &mut started.branches),
            (&mut self.names.users, &mut started.users),
            (&mut self.names.teams, &mut started.teams),
        ]
        .into_iter()
        .zip(wanted)
        {
            if wanted && fetch.is_none() {
                self.next_fetch_id += 1;
                *fetch = Some(Fetch::Loading(self.next_fetch_id));
                *id = Some(self.next_fetch_id);
            }
        }
        started
    }

    /// PRs whose details haven't been requested yet, as many as there are free fetch slots, with
    /// their fetch ids. Marks them as loading, so the caller must start the fetches.
    pub fn details_to_fetch(&mut self) -> Vec<(u64, FetchId)> {
        let order = self.numbers_from_highlight();
        start_fetches(
            &mut self.details,
            &mut self.next_fetch_id,
            order,
            MAX_DETAIL_FETCHES,
        )
    }

    /// PRs whose diffs haven't been requested yet, as many as there are free fetch slots, with
    /// their fetch ids. Marks them as loading, so the caller must start the fetches. None while the
    /// Diff pane is hidden, as nothing shows them; they are fetched once it is shown again.
    ///
    /// Only the [`MAX_CACHED_DIFFS`] PRs nearest the highlight are fetched. Past that many diffs,
    /// those outside this window are dropped (fetches still running are kept, and are dropped on
    /// the next call after they finish), so a dropped diff isn't fetched again until the
    /// highlight comes near it.
    pub fn diffs_to_fetch(&mut self) -> Vec<(u64, FetchId)> {
        if self.pane_hidden(Pane::Diff) {
            return Vec::new();
        }
        let mut order = self.numbers_from_highlight();
        order.truncate(MAX_CACHED_DIFFS);
        if self.diffs.len() > MAX_CACHED_DIFFS {
            let window: HashSet<u64> = order.iter().copied().collect();
            self.diffs.retain(|number, fetch| {
                window.contains(number) || matches!(fetch, Fetch::Loading(_))
            });
        }
        start_fetches(
            &mut self.diffs,
            &mut self.next_fetch_id,
            order,
            MAX_DIFF_FETCHES,
        )
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
}
