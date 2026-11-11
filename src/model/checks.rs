//! Working out the state of a PR's checks.

use std::collections::BTreeMap;

use crate::gh::{Check, Conclusion, ContextState, Mergeable, ReviewDecision, RunStatus};

/// The state of a single check, or of all a PR's checks together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Passing,
    Pending,
    Failing,
    /// The PR has no checks.
    None,
    /// The PR has merge conflicts, whatever its checks say.
    Conflict,
}

impl CheckState {
    /// Lower is better: passing 0, failing and conflict 2, everything else 1.
    pub fn rank(self) -> u8 {
        match self {
            Self::Passing => 0,
            Self::Pending | Self::None => 1,
            Self::Failing | Self::Conflict => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Passing => "✓ passing",
            Self::Pending => "… pending",
            Self::Failing => "✗ failing",
            Self::None => "· no checks",
            Self::Conflict => "✗ conflict",
        }
    }
}

/// The latest run of one check.
#[derive(Debug, PartialEq, Eq)]
pub struct CheckResult {
    /// `workflowName/name` for a check run, `context` for a status context.
    pub key: String,
    pub state: CheckState,
    /// The page to open for it, if `gh` gave one.
    pub url: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Checks {
    pub state: CheckState,
    /// The latest run per key, sorted by key.
    pub results: Vec<CheckResult>,
}

impl Checks {
    /// The results with the worst first, then by key. This is the order of the checks pane; the
    /// PR list sorts the other way round.
    pub fn worst_first(&self) -> Vec<&CheckResult> {
        let mut results: Vec<&CheckResult> = self.results.iter().collect();
        // The sort is stable, so equal ranks stay in key order.
        results.sort_by_key(|result| std::cmp::Reverse(result.state.rank()));
        results
    }
}

pub fn check_state(check: &Check) -> CheckState {
    match check {
        Check::CheckRun(run) if run.status != RunStatus::Completed => CheckState::Pending,
        Check::CheckRun(run) => match run.conclusion {
            Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped => CheckState::Passing,
            Conclusion::Other => CheckState::Failing,
        },
        Check::StatusContext(ctx) => match ctx.state {
            ContextState::Success => CheckState::Passing,
            ContextState::Pending | ContextState::Expected => CheckState::Pending,
            ContextState::Other => CheckState::Failing,
        },
        Check::Unknown => CheckState::Pending,
    }
}

/// Keeps the latest run of each check and works out the overall state. A conflict outranks the
/// check results.
pub fn classify_checks(mergeable: Mergeable, rollup: &[Check]) -> Checks {
    let mut latest: BTreeMap<String, &Check> = BTreeMap::new();
    for check in rollup {
        let (key, started_at) = match check {
            Check::CheckRun(run) => (
                format!("{}/{}", run.workflow_name, run.name),
                &run.started_at,
            ),
            Check::StatusContext(ctx) => (ctx.context.clone(), &ctx.started_at),
            Check::Unknown => ("(unrecognised check)".to_string(), &None),
        };
        // A missing start time sorts as earliest; on a tie the later entry wins.
        let newer = latest
            .get(&key)
            .is_none_or(|seen| started_at >= started_at_of(seen));
        if newer {
            latest.insert(key, check);
        }
    }
    let results: Vec<CheckResult> = latest
        .into_iter()
        .map(|(key, check)| CheckResult {
            key,
            state: check_state(check),
            url: check_url(check),
        })
        .collect();

    let any = |state| results.iter().any(|r| r.state == state);
    let state = if mergeable == Mergeable::Conflicting {
        CheckState::Conflict
    } else if any(CheckState::Failing) {
        CheckState::Failing
    } else if mergeable == Mergeable::Unknown || any(CheckState::Pending) {
        // Until GitHub has checked for a conflict, passing or no checks may not be the truth.
        CheckState::Pending
    } else if results.is_empty() {
        CheckState::None
    } else {
        CheckState::Passing
    };
    Checks { state, results }
}

/// The page of a check, or `None` if it has none or `gh` left it empty.
fn check_url(check: &Check) -> Option<String> {
    let url = match check {
        Check::CheckRun(run) => &run.details_url,
        Check::StatusContext(ctx) => &ctx.target_url,
        Check::Unknown => &None,
    };
    url.clone().filter(|url| !url.is_empty())
}

fn started_at_of(check: &Check) -> &Option<String> {
    match check {
        Check::CheckRun(run) => &run.started_at,
        Check::StatusContext(ctx) => &ctx.started_at,
        Check::Unknown => &None,
    }
}

pub fn review_label(review_decision: ReviewDecision) -> &'static str {
    match review_decision {
        ReviewDecision::Approved => "✓ approved",
        ReviewDecision::ChangesRequested => "✗ changes",
        ReviewDecision::Other => "· needed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The checks are built from JSON, as gh prints them.
    fn check_run(name: &str, status: &str, conclusion: &str, started_at: Option<&str>) -> Check {
        serde_json::from_value(serde_json::json!({
            "__typename": "CheckRun",
            "name": name,
            "workflowName": "CI",
            "status": status,
            "conclusion": conclusion,
            "startedAt": started_at,
        }))
        .unwrap()
    }

    /// A check run that finished with `conclusion`.
    fn completed(name: &str, conclusion: &str) -> Check {
        check_run(name, "COMPLETED", conclusion, None)
    }

    /// A finished check run that started at `started_at`.
    fn completed_at(name: &str, conclusion: &str, started_at: &str) -> Check {
        check_run(name, "COMPLETED", conclusion, Some(started_at))
    }

    fn in_progress(name: &str) -> Check {
        check_run(name, "IN_PROGRESS", "", None)
    }

    /// A commit status, as an older CI reports.
    fn status(context: &str, state: &str) -> Check {
        status_at_time(context, state, None)
    }

    /// A commit status that started at `started_at`.
    fn status_at(context: &str, state: &str, started_at: &str) -> Check {
        status_at_time(context, state, Some(started_at))
    }

    fn status_at_time(context: &str, state: &str, started_at: Option<&str>) -> Check {
        serde_json::from_value(serde_json::json!({
            "__typename": "StatusContext",
            "context": context,
            "state": state,
            "startedAt": started_at,
        }))
        .unwrap()
    }

    #[test]
    fn single_check_state() {
        use CheckState::{Failing, Passing, Pending};
        let cases = [
            // A check run that has not finished is pending, whatever it says it concluded.
            (in_progress("a"), Pending, "in progress"),
            (check_run("a", "QUEUED", "SUCCESS", None), Pending, "queued"),
            (completed("a", "SUCCESS"), Passing, "SUCCESS"),
            (completed("a", "NEUTRAL"), Passing, "NEUTRAL"),
            (completed("a", "SKIPPED"), Passing, "SKIPPED"),
            (completed("a", "FAILURE"), Failing, "FAILURE"),
            (completed("a", "CANCELLED"), Failing, "CANCELLED"),
            (completed("a", "TIMED_OUT"), Failing, "TIMED_OUT"),
            (
                completed("a", "ACTION_REQUIRED"),
                Failing,
                "ACTION_REQUIRED",
            ),
            (completed("a", ""), Failing, "no conclusion"),
            (status("c", "SUCCESS"), Passing, "status SUCCESS"),
            (status("c", "PENDING"), Pending, "status PENDING"),
            (status("c", "EXPECTED"), Pending, "status EXPECTED"),
            (status("c", "FAILURE"), Failing, "status FAILURE"),
            (status("c", "ERROR"), Failing, "status ERROR"),
        ];
        for (check, expected, what) in cases {
            assert_eq!(check_state(&check), expected, "{what}");
        }
    }

    #[test]
    fn keeps_latest_run_per_key() {
        let checks = classify_checks(
            Mergeable::Other,
            &[
                completed_at("build", "FAILURE", "2026-01-01T10:00:00Z"),
                completed_at("build", "SUCCESS", "2026-01-01T11:00:00Z"),
                completed("build", "FAILURE"),
                status("ci/legacy", "PENDING"),
                status_at("ci/legacy", "SUCCESS", "2026-01-01T09:00:00Z"),
            ],
        );
        assert_eq!(
            checks.results,
            vec![
                CheckResult {
                    key: "CI/build".to_string(),
                    state: CheckState::Passing,
                    url: None,
                },
                CheckResult {
                    key: "ci/legacy".to_string(),
                    state: CheckState::Passing,
                    url: None,
                },
            ]
        );
        assert_eq!(checks.state, CheckState::Passing);
    }

    #[test]
    fn checks_pane_lists_the_worst_first() {
        let checks = classify_checks(
            Mergeable::Other,
            &[
                completed("a", "SUCCESS"),
                in_progress("b"),
                completed("c", "FAILURE"),
                completed("d", "SUCCESS"),
                status("e", "ERROR"),
                check_run("f", "QUEUED", "", None),
            ],
        );
        let keys: Vec<&str> = checks
            .worst_first()
            .into_iter()
            .map(|result| result.key.as_str())
            .collect();
        assert_eq!(keys, ["CI/c", "e", "CI/b", "CI/f", "CI/a", "CI/d"]);
    }

    #[test]
    fn overall_state_priority() {
        let pass = completed("a", "SUCCESS");
        let pend = in_progress("b");
        let fail = completed("c", "FAILURE");
        let unknown = Check::Unknown;
        let state = |mergeable, checks: &[&Check]| {
            let checks: Vec<Check> = checks.iter().map(|c| (*c).clone()).collect();
            classify_checks(mergeable, &checks).state
        };
        assert_eq!(
            state(Mergeable::Conflicting, &[&pass]),
            CheckState::Conflict
        );
        assert_eq!(state(Mergeable::Conflicting, &[]), CheckState::Conflict);
        assert_eq!(state(Mergeable::Other, &[]), CheckState::None);
        assert_eq!(
            state(Mergeable::Other, &[&pass, &pend, &fail]),
            CheckState::Failing
        );
        assert_eq!(
            state(Mergeable::Other, &[&pass, &pend]),
            CheckState::Pending
        );
        assert_eq!(state(Mergeable::Other, &[&pass]), CheckState::Passing);
        // Until GitHub knows whether there is a conflict, a PR is not yet passing.
        assert_eq!(state(Mergeable::Unknown, &[&pass]), CheckState::Pending);
        assert_eq!(state(Mergeable::Unknown, &[]), CheckState::Pending);
        assert_eq!(state(Mergeable::Unknown, &[&fail]), CheckState::Failing);
        // An unknown check keeps the PR from passing, and is not `no checks` on its own either,
        // but a failing check still outranks it.
        assert_eq!(
            state(Mergeable::Other, &[&pass, &unknown]),
            CheckState::Pending
        );
        assert_eq!(state(Mergeable::Other, &[&unknown]), CheckState::Pending);
        assert_eq!(
            state(Mergeable::Other, &[&unknown, &fail]),
            CheckState::Failing
        );
        // It is listed, so the checks pane shows why.
        let checks = classify_checks(Mergeable::Other, &[pass.clone(), unknown.clone()]);
        assert_eq!(checks.results.len(), 2);
    }
}
