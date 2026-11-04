//! Approving and merging a batch of PRs, and the dialog that asks what to do with them.

use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::model::{CheckState, one_line};

/// Approves PRs, several at once up to a limit, and merges each approved one, one at a time, if
/// asked to. A PR is merged as soon as it is approved.
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

impl BatchItem {
    /// The status of the step the PR is on, or of the approval that stopped it before merging.
    pub fn current_status(&self) -> &ItemStatus {
        self.merging.as_ref().unwrap_or(&self.approval)
    }

    /// The stage the PR is on, as shown in the progress table: waiting, approving, waiting to
    /// merge, merging or done, or how it stopped.
    pub fn label(&self) -> String {
        let (step, status) = match &self.merging {
            Some(status) => ("merge", status),
            None => ("approve", &self.approval),
        };
        match status {
            ItemStatus::Pending if step == "merge" => "· waiting to merge".to_string(),
            ItemStatus::Pending => "· waiting".to_string(),
            ItemStatus::Working if step == "merge" => "… merging".to_string(),
            ItemStatus::Working => "… approving".to_string(),
            ItemStatus::Done => "✓ done".to_string(),
            ItemStatus::AutoMerge => "⏲ auto-merge queued".to_string(),
            ItemStatus::Failed(error) => format!("✗ {step} failed: {error}"),
            ItemStatus::Cancelled => "· cancelled".to_string(),
            ItemStatus::Interrupted => "? interrupted - check GitHub".to_string(),
        }
    }

    fn steps(&self) -> [Option<&ItemStatus>; 2] {
        [Some(&self.approval), self.merging.as_ref()]
    }
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
    /// The batch was cancelled before this step started.
    Cancelled,
    /// The batch was cancelled while this step was running. It can't be stopped, so GitHub may
    /// or may not have done it.
    Interrupted,
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
    /// Stops the steps that haven't started, and marks those running as interrupted, as they
    /// can't be stopped. A cancelled Approve & merge doesn't merge the PRs approved so far, other
    /// than one already being merged.
    pub fn cancel(&mut self) {
        for item in &mut self.items {
            for status in [Some(&mut item.approval), item.merging.as_mut()]
                .into_iter()
                .flatten()
            {
                match status {
                    ItemStatus::Pending => *status = ItemStatus::Cancelled,
                    ItemStatus::Working => *status = ItemStatus::Interrupted,
                    _ => {}
                }
            }
        }
    }

    /// Queues the PR for merging once its approval has succeeded, if the batch merges. A PR can
    /// be merged while others are still being approved.
    pub fn queue_merge(&mut self, number: u64) {
        let merge = self.merge;
        if let Some(item) = self.items.iter_mut().find(|item| item.pr.number == number)
            && merge
            && item.approval.is_ok()
        {
            item.merging = Some(ItemStatus::Pending);
        }
    }

    /// Whether the PR has nothing left to do: its merge is finished, or for a batch that doesn't
    /// merge, or a PR that wasn't approved, its approval is.
    fn is_item_finished(&self, item: &BatchItem) -> bool {
        match &item.merging {
            Some(status) => status.is_finished(),
            None => item.approval.is_finished(),
        }
    }

    /// Whether every PR has nothing left to do.
    pub fn is_finished(&self) -> bool {
        self.items.iter().all(|item| self.is_item_finished(item))
    }

    /// How many PRs have nothing left to do, out of how many.
    pub fn progress(&self) -> (usize, usize) {
        let finished = self
            .items
            .iter()
            .filter(|item| self.is_item_finished(item))
            .count();
        (finished, self.items.len())
    }

    /// How the batch went, once it has finished.
    pub fn summary(&self) -> Summary {
        let mut failures = Vec::new();
        for item in &self.items {
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
        let approved = self
            .items
            .iter()
            .filter(|item| item.approval.is_ok())
            .count();
        let merges: Vec<&ItemStatus> = self
            .items
            .iter()
            .filter_map(|item| item.merging.as_ref())
            .collect();
        let merged = self.merge.then(|| {
            let ok = merges.iter().filter(|status| status.is_ok()).count();
            (ok, merges.len())
        });
        let cancelled = self
            .items
            .iter()
            .filter(|item| item.steps().contains(&Some(&ItemStatus::Cancelled)))
            .count();
        let interrupted = self
            .items
            .iter()
            .filter(|item| item.steps().contains(&Some(&ItemStatus::Interrupted)))
            .map(|item| item.pr.number)
            .collect();
        let auto_merge = self
            .items
            .iter()
            .filter(|item| item.merging == Some(ItemStatus::AutoMerge))
            .map(|item| item.pr.number)
            .collect();
        Summary {
            approved,
            total: self.items.len(),
            merged,
            failures,
            auto_merge,
            cancelled,
            interrupted,
        }
    }
}

#[derive(Debug)]
pub struct Summary {
    pub approved: usize,
    pub total: usize,
    /// How many were merged or queued for auto-merge, out of how many were due to merge, which
    /// includes cancelled and interrupted ones. `None` when the batch didn't merge.
    pub merged: Option<(usize, usize)>,
    /// PR numbers, the step that failed and the error on one line.
    pub failures: Vec<(u64, &'static str, String)>,
    /// PRs that will merge once their checks pass.
    pub auto_merge: Vec<u64>,
    /// How many PRs had a step cancelled before it started.
    pub cancelled: usize,
    /// PRs with a step that was running when the batch was cancelled, so nobody knows whether
    /// it happened.
    pub interrupted: Vec<u64>,
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

/// Whether a PR with these checks is merged with auto-merge, so GitHub merges it once they
/// pass. Only pending checks count: a PR with no checks is merged straight away.
pub fn auto_merge(checks: CheckState) -> bool {
    checks == CheckState::Pending
}

/// A finished step's status: `ok` on success, or the error on one line.
pub fn finished_status(result: Result<()>, ok: ItemStatus) -> ItemStatus {
    match result {
        Ok(()) => ok,
        Err(e) => ItemStatus::Failed(one_line(&e.to_string())),
    }
}

/// Moves between the dialog's choices, and returns the choice made, if any.
pub fn on_confirm_key(confirm: &mut Confirm, key: KeyEvent) -> Option<Choice> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(number: u64, approval: ItemStatus, merging: Option<ItemStatus>) -> BatchItem {
        BatchItem {
            pr: ConfirmPr {
                number,
                checks: CheckState::Passing,
                title: String::new(),
            },
            approval,
            merging,
        }
    }

    #[test]
    fn the_summary_counts_each_step_and_names_the_failures() {
        let batch = Batch {
            items: vec![
                item(1, ItemStatus::Done, Some(ItemStatus::Done)),
                item(2, ItemStatus::Done, Some(ItemStatus::AutoMerge)),
                item(
                    3,
                    ItemStatus::Done,
                    Some(ItemStatus::Failed("conflict".into())),
                ),
                item(4, ItemStatus::Failed("no".into()), None),
            ],
            merge: true,
        };
        let summary = batch.summary();
        assert_eq!((summary.approved, summary.total), (3, 4));
        assert_eq!(summary.merged, Some((2, 3)));
        assert_eq!(summary.auto_merge, [2]);
        assert_eq!(
            summary.failures,
            [
                (3, "merge", "conflict".to_string()),
                (4, "approve", "no".to_string())
            ]
        );
    }

    #[test]
    fn an_item_is_labelled_with_its_stage() {
        let label = |approval, merging| item(1, approval, merging).label();
        assert_eq!(label(ItemStatus::Pending, None), "· waiting");
        assert_eq!(label(ItemStatus::Working, None), "… approving");
        assert_eq!(
            label(ItemStatus::Done, Some(ItemStatus::Pending)),
            "· waiting to merge"
        );
        assert_eq!(
            label(ItemStatus::Done, Some(ItemStatus::Working)),
            "… merging"
        );
        assert_eq!(label(ItemStatus::Done, Some(ItemStatus::Done)), "✓ done");
        assert_eq!(
            label(ItemStatus::Failed("no".into()), None),
            "✗ approve failed: no"
        );
        assert_eq!(
            label(ItemStatus::Done, Some(ItemStatus::Failed("busy".into()))),
            "✗ merge failed: busy"
        );
    }

    #[test]
    fn a_batch_that_didnt_merge_has_no_merge_count() {
        let batch = Batch {
            items: vec![item(1, ItemStatus::Done, None)],
            merge: false,
        };
        assert_eq!(batch.summary().merged, None);
    }
}
