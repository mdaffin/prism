//! Approving and merging a batch of PRs, and the dialog that asks what to do with them.

use color_eyre::eyre::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::one_line;
use crate::model::CheckState;

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

impl BatchItem {
    /// The status of the step the PR is on, or of the approval that stopped it before merging.
    pub fn current_status(&self) -> &ItemStatus {
        self.merging.as_ref().unwrap_or(&self.approval)
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

    /// The status as shown in the progress table.
    pub fn label(&self) -> String {
        match self {
            Self::Pending => "· pending".to_string(),
            Self::Working => "… working".to_string(),
            Self::Done => "✓ done".to_string(),
            Self::AutoMerge => "⏲ auto-merge queued".to_string(),
            Self::Failed(error) => format!("✗ failed: {error}"),
            Self::Cancelled => "· cancelled".to_string(),
            Self::Interrupted => "? interrupted - check GitHub".to_string(),
        }
    }
}

impl Batch {
    /// Whether the batch has moved on to merging.
    pub fn is_merging(&self) -> bool {
        self.items.iter().any(|item| item.merging.is_some())
    }

    /// Stops the steps that haven't started, and marks those running as interrupted, as they
    /// can't be stopped. A cancelled Approve & merge doesn't merge the PRs approved so far.
    pub fn cancel(&mut self) {
        let was_merging = self.is_merging();
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
            if self.merge && !was_merging && item.approval.is_ok() {
                item.merging = Some(ItemStatus::Cancelled);
            }
        }
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
        // Merges that were cancelled before they started weren't tried.
        let merged = self.merge.then(|| {
            let tried = merges
                .iter()
                .filter(|status| ***status != ItemStatus::Cancelled);
            let ok = tried.clone().filter(|status| status.is_ok()).count();
            (ok, tried.count())
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
    /// How many were merged or queued for auto-merge, out of how many were tried. `None` when
    /// the batch didn't merge.
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
    fn a_batch_that_didnt_merge_has_no_merge_count() {
        let batch = Batch {
            items: vec![item(1, ItemStatus::Done, None)],
            merge: false,
        };
        assert_eq!(batch.summary().merged, None);
    }
}
