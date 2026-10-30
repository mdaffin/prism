//! PR logic with no I/O.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::gh::Check;

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
}

#[derive(Debug, PartialEq, Eq)]
pub struct Checks {
    pub state: CheckState,
    /// The latest run per key, sorted by key. Read by the checks pane (M2).
    #[allow(dead_code)]
    pub results: Vec<CheckResult>,
}

pub fn check_state(check: &Check) -> CheckState {
    match check {
        Check::CheckRun(run) if run.status != "COMPLETED" => CheckState::Pending,
        Check::CheckRun(run) => match run.conclusion.as_str() {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => CheckState::Passing,
            _ => CheckState::Failing,
        },
        Check::StatusContext(ctx) => match ctx.state.as_str() {
            "SUCCESS" => CheckState::Passing,
            "PENDING" | "EXPECTED" => CheckState::Pending,
            _ => CheckState::Failing,
        },
    }
}

/// Keeps the latest run of each check and works out the overall state. A conflict outranks the
/// check results.
pub fn classify_checks(mergeable: &str, rollup: &[Check]) -> Checks {
    let mut latest: BTreeMap<String, &Check> = BTreeMap::new();
    for check in rollup {
        let (key, started_at) = match check {
            Check::CheckRun(run) => (
                format!("{}/{}", run.workflow_name, run.name),
                &run.started_at,
            ),
            Check::StatusContext(ctx) => (ctx.context.clone(), &ctx.started_at),
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
        })
        .collect();

    let any = |state| results.iter().any(|r| r.state == state);
    let state = if mergeable == "CONFLICTING" {
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

fn started_at_of(check: &Check) -> &Option<String> {
    match check {
        Check::CheckRun(run) => &run.started_at,
        Check::StatusContext(ctx) => &ctx.started_at,
    }
}

pub fn review_label(review_decision: &str) -> &'static str {
    match review_decision {
        "APPROVED" => "✓ approved",
        "CHANGES_REQUESTED" => "✗ changes",
        _ => "· needed",
    }
}

static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
static TICKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?<ticket>[A-Z][A-Z0-9]+-\d+)[:\s]*(?<rest>.*)$").unwrap());

/// Removes HTML comments and runs of blank lines from a PR or comment body.
pub fn clean_body(body: Option<&str>) -> String {
    let body = body.unwrap_or_default().replace("\r\n", "\n");
    let body = HTML_COMMENT.replace_all(&body, "");
    let body = BLANK_LINES.replace_all(&body, "\n\n");
    body.trim().to_string()
}

/// Splits a leading ticket such as `ABC-123:` off a title. Returns an empty ticket and the full
/// title when there is none. Read by the ticket column (M4).
#[allow(dead_code)]
pub fn split_ticket(title: &str) -> (&str, &str) {
    match TICKET.captures(title) {
        Some(caps) => (
            caps.name("ticket").unwrap().as_str(),
            caps.name("rest").unwrap().as_str(),
        ),
        None => ("", title),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh::{CheckRun, StatusContext};

    fn run(name: &str, status: &str, conclusion: &str, started_at: Option<&str>) -> Check {
        Check::CheckRun(CheckRun {
            name: name.to_string(),
            workflow_name: "CI".to_string(),
            status: status.to_string(),
            conclusion: conclusion.to_string(),
            started_at: started_at.map(str::to_string),
        })
    }

    fn ctx(context: &str, state: &str, started_at: Option<&str>) -> Check {
        Check::StatusContext(StatusContext {
            context: context.to_string(),
            state: state.to_string(),
            started_at: started_at.map(str::to_string),
        })
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
            "MERGEABLE",
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
                    state: CheckState::Passing
                },
                CheckResult {
                    key: "ci/legacy".to_string(),
                    state: CheckState::Passing
                },
            ]
        );
        assert_eq!(checks.state, CheckState::Passing);
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
        assert_eq!(state("CONFLICTING", &[&pass]), CheckState::Conflict);
        assert_eq!(state("CONFLICTING", &[]), CheckState::Conflict);
        assert_eq!(state("MERGEABLE", &[]), CheckState::None);
        assert_eq!(
            state("MERGEABLE", &[&pass, &pend, &fail]),
            CheckState::Failing
        );
        assert_eq!(state("UNKNOWN", &[&pass, &pend]), CheckState::Pending);
        assert_eq!(state("MERGEABLE", &[&pass]), CheckState::Passing);
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
        assert_eq!(review_label("APPROVED"), "✓ approved");
        assert_eq!(review_label("CHANGES_REQUESTED"), "✗ changes");
        assert_eq!(review_label("REVIEW_REQUIRED"), "· needed");
        assert_eq!(review_label(""), "· needed");
    }

    #[test]
    fn cleans_body() {
        assert_eq!(clean_body(None), "");
        assert_eq!(
            clean_body(Some(
                "<!-- a\nmulti-line -->Hello<!-- b -->\n\n\n\n\nWorld<!-- c -->\n"
            )),
            "Hello\n\nWorld"
        );
        assert_eq!(clean_body(Some("one\r\n\r\n\r\n\r\ntwo")), "one\n\ntwo");
        assert_eq!(clean_body(Some("keep\n\nthis")), "keep\n\nthis");
    }

    #[test]
    fn splits_ticket() {
        assert_eq!(split_ticket("ABC-123: Update x"), ("ABC-123", "Update x"));
        assert_eq!(split_ticket("AB2-7 Update x"), ("AB2-7", "Update x"));
        assert_eq!(split_ticket("Update ABC-123"), ("", "Update ABC-123"));
        assert_eq!(split_ticket("A-1: too short"), ("", "A-1: too short"));
        assert_eq!(split_ticket("abc-1: lower"), ("", "abc-1: lower"));
    }
}
