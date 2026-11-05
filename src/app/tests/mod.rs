use super::work::StatusFetch;
use super::*;
use crate::gh::{PrStatus, ReviewDecision};
use crate::testing::{
    PrBuilder, app_with, ctrl, ctrl_enter, ctrl_g, key, passing_run, pr_details, press,
    refresh_and_load, search_and_load, select_rows, status_context, with_check_rows,
    with_comment_rows,
};
use crossterm::event::{KeyCode, KeyModifiers};

fn pr(number: u64, state: &str) -> Pr {
    PrBuilder::new(number).state(state).build()
}

fn prs(numbers: impl IntoIterator<Item = u64>) -> Vec<Pr> {
    numbers
        .into_iter()
        .map(|number| pr(number, "OPEN"))
        .collect()
}

fn app_with_rows(count: u64) -> App {
    app_with(prs(1..=count))
}

/// The PR numbers of the fetches an app started.
fn started(fetches: Vec<(u64, FetchId)>) -> Vec<u64> {
    fetches.into_iter().map(|(number, _)| number).collect()
}

fn numbers(app: &App) -> Vec<u64> {
    app.rows.iter().map(|row| row.pr.number).collect()
}

/// Presses Tab until `pane` has focus.
pub(super) fn focus(app: &mut App, pane: Pane) {
    for _ in 0..5 {
        if app.focus == pane {
            return;
        }
        key(app, KeyCode::Tab);
    }
    panic!("Tab never reached the pane");
}

/// An open PR whose checks are passing, pending or absent.
pub(super) fn pr_with_checks(number: u64, checks: &str) -> Pr {
    let rollup = match checks {
        "passing" => serde_json::json!([passing_run()]),
        "pending" => serde_json::json!([status_context("PENDING")]),
        _ => serde_json::json!([]),
    };
    PrBuilder::new(number).checks(rollup).build()
}

pub(super) fn titled(number: u64, title: &str) -> Pr {
    PrBuilder::new(number).title(title).build()
}

/// Three open PRs titled "Bump alpha", "Bump beta" and "Update gamma".
pub(super) fn app_with_titles() -> App {
    app_with(vec![
        titled(1, "Bump alpha"),
        titled(2, "Bump beta"),
        titled(3, "Update gamma"),
    ])
}

pub(super) fn visible_numbers(app: &App) -> Vec<u64> {
    app.visible_rows().map(|row| row.pr.number).collect()
}

/// Starts a timed refresh, as the timer and the loop would.
pub(super) fn start_status_refresh(app: &mut App) -> Option<StatusFetch> {
    app.update(Msg::RefreshDue);
    app.status_to_fetch()
}

pub(super) fn status(
    number: u64,
    state: &str,
    passing: bool,
    review: &str,
    updated: &str,
) -> PrStatus {
    let rollup = if passing {
        serde_json::json!([passing_run()])
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

pub(super) fn update(statuses: Vec<PrStatus>) -> StatusUpdate {
    StatusUpdate {
        statuses,
        failure: None,
    }
}

pub(super) const UNCHANGED: &str = "2026-01-01T00:00:00Z";

pub(super) fn approve(app: &mut App, number: u64, result: Result<()>) {
    let epoch = app.batch_epoch();
    app.update(Msg::Approved(epoch, number, result));
}

/// Selects the first `count` rows and chooses Approve.
pub(super) fn start_approving(app: &mut App, count: usize) {
    for _ in 0..count {
        press(app, " j");
    }
    ctrl_g(app);
    key(app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Running(_)));
}

pub(super) fn confirm_numbers(app: &App) -> Vec<u64> {
    confirm(app).prs.iter().map(|pr| pr.number).collect()
}

pub(super) fn confirm(app: &App) -> &Confirm {
    match &app.mode {
        Mode::Confirm(confirm) => confirm,
        _ => panic!("the confirm dialog isn't open"),
    }
}

mod batch;
mod fetching;
mod filter;
mod open_copy;
mod panes;
mod refresh;
mod search;
mod selection;
mod sort;
