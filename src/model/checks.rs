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
    } else if results.is_empty() {
        CheckState::None
    } else if any(CheckState::Failing) {
        CheckState::Failing
    } else if any(CheckState::Pending) {
        CheckState::Pending
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
    fn run(name: &str, status: &str, conclusion: &str, started_at: Option<&str>) -> Check {
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

    fn ctx(context: &str, state: &str, started_at: Option<&str>) -> Check {
        serde_json::from_value(serde_json::json!({
            "__typename": "StatusContext",
            "context": context,
            "state": state,
            "startedAt": started_at,
        }))
        .unwrap()
    }

    fn decision(value: &str) -> ReviewDecision {
        serde_json::from_value(serde_json::json!(value)).unwrap()
    }

    #[test]
    fn an_unknown_check_keeps_the_pr_from_passing() {
        let checks = classify_checks(
            Mergeable::Other,
            &[ctx("ci/legacy", "SUCCESS", None), Check::Unknown],
        );
        assert_eq!(checks.state, CheckState::Pending);
        // It is listed, so the checks pane shows why.
        assert_eq!(checks.results.len(), 2);

        // On its own it is pending too, not "no checks".
        let checks = classify_checks(Mergeable::Other, &[Check::Unknown]);
        assert_eq!(checks.state, CheckState::Pending);
        // A failing check still outranks it.
        let checks = classify_checks(
            Mergeable::Other,
            &[Check::Unknown, ctx("a", "FAILURE", None)],
        );
        assert_eq!(checks.state, CheckState::Failing);
    }

    #[test]
    fn check_run_state() {
        assert_eq!(
            check_state(&run("a", "IN_PROGRESS", "", None)),
            CheckState::Pending
        );
        assert_eq!(
            check_state(&run("a", "QUEUED", "SUCCESS", None)),
            CheckState::Pending
        );
        for conclusion in ["SUCCESS", "NEUTRAL", "SKIPPED"] {
            let check = run("a", "COMPLETED", conclusion, None);
            assert_eq!(check_state(&check), CheckState::Passing, "{conclusion}");
        }
        for conclusion in ["FAILURE", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED", ""] {
            let check = run("a", "COMPLETED", conclusion, None);
            assert_eq!(check_state(&check), CheckState::Failing, "{conclusion}");
        }
    }

    #[test]
    fn status_context_state() {
        assert_eq!(check_state(&ctx("c", "SUCCESS", None)), CheckState::Passing);
        assert_eq!(check_state(&ctx("c", "PENDING", None)), CheckState::Pending);
        assert_eq!(
            check_state(&ctx("c", "EXPECTED", None)),
            CheckState::Pending
        );
        assert_eq!(check_state(&ctx("c", "FAILURE", None)), CheckState::Failing);
        assert_eq!(check_state(&ctx("c", "ERROR", None)), CheckState::Failing);
    }

    #[test]
    fn keeps_latest_run_per_key() {
        let checks = classify_checks(
            Mergeable::Other,
            &[
                run(
                    "build",
                    "COMPLETED",
                    "FAILURE",
                    Some("2026-01-01T10:00:00Z"),
                ),
                run(
                    "build",
                    "COMPLETED",
                    "SUCCESS",
                    Some("2026-01-01T11:00:00Z"),
                ),
                run("build", "COMPLETED", "FAILURE", None),
                ctx("ci/legacy", "PENDING", None),
                ctx("ci/legacy", "SUCCESS", Some("2026-01-01T09:00:00Z")),
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
                run("a", "COMPLETED", "SUCCESS", None),
                run("b", "IN_PROGRESS", "", None),
                run("c", "COMPLETED", "FAILURE", None),
                run("d", "COMPLETED", "SUCCESS", None),
                ctx("e", "ERROR", None),
                run("f", "QUEUED", "", None),
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
        let pass = run("a", "COMPLETED", "SUCCESS", None);
        let pend = run("b", "IN_PROGRESS", "", None);
        let fail = run("c", "COMPLETED", "FAILURE", None);
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
    }

    #[test]
    fn ranks() {
        assert_eq!(CheckState::Passing.rank(), 0);
        assert_eq!(CheckState::Pending.rank(), 1);
        assert_eq!(CheckState::None.rank(), 1);
        assert_eq!(CheckState::Failing.rank(), 2);
        assert_eq!(CheckState::Conflict.rank(), 2);
    }

    #[test]
    fn review_labels() {
        assert_eq!(review_label(decision("APPROVED")), "✓ approved");
        assert_eq!(review_label(decision("CHANGES_REQUESTED")), "✗ changes");
        assert_eq!(review_label(decision("REVIEW_REQUIRED")), "· needed");
        assert_eq!(review_label(decision("")), "· needed");
    }
}
