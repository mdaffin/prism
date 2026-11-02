//! The background work the app asks for: what to fetch or run next, and what was fetched.
//! `update` decides, and the caller in `main` starts it and sends back a `Msg`.

use std::collections::HashMap;

use ratatui::text::Text;

use super::{App, Mode, RefreshId};
use crate::batch::{ItemStatus, auto_merge};
use crate::gh::{PrDetails, PrState};

/// The most detail fetches to run at once.
const MAX_DETAIL_FETCHES: usize = 4;

/// The most diffs fetched at once.
const MAX_DIFF_FETCHES: usize = 4;

/// The most approvals to run at once.
const MAX_APPROVALS: usize = 4;

/// The most row refreshes to run at once.
const MAX_REFRESHES: usize = 4;

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
    pub body: Text<'static>,
    /// Each comment's body, rendered the same way and in the same order as `details.comments`.
    pub comment_bodies: Vec<Text<'static>>,
}

/// A PR's diff with gh's colours.
pub type Diff = Text<'static>;

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

/// Drops the failed fetches, so they are tried again.
pub(super) fn forget_failed<T>(fetches: &mut HashMap<u64, Fetch<T>>) {
    fetches.retain(|_, fetch| !matches!(fetch, Fetch::Failed(_)));
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
    /// their fetch ids. Marks them as loading, so the caller must start the fetches.
    pub fn diffs_to_fetch(&mut self) -> Vec<(u64, FetchId)> {
        let order = self.numbers_from_highlight();
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
