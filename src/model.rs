//! PR logic with no I/O.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use ratatui::text::{Line, Span, Text};
use regex::Regex;
use unicode_width::UnicodeWidthChar;

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

/// How long before `now` a timestamp such as `2026-01-02T03:04:05Z` was, like `3 hours ago`.
/// A timestamp that can't be read is shown as it is.
pub fn ago(timestamp: &str, now: jiff::Timestamp) -> String {
    let Ok(then) = timestamp.parse::<jiff::Timestamp>() else {
        return timestamp.to_string();
    };
    let seconds = now.as_second() - then.as_second();
    let units = [
        (365 * 24 * 3600, "year"),
        (30 * 24 * 3600, "month"),
        (7 * 24 * 3600, "week"),
        (24 * 3600, "day"),
        (3600, "hour"),
        (60, "minute"),
    ];
    for (size, unit) in units {
        if seconds >= size {
            let n = seconds / size;
            return format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
        }
    }
    "just now".to_string()
}

pub fn review_label(review_decision: ReviewDecision) -> &'static str {
    match review_decision {
        ReviewDecision::Approved => "✓ approved",
        ReviewDecision::ChangesRequested => "✗ changes",
        ReviewDecision::Other => "· needed",
    }
}

static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// Removes HTML comments and runs of blank lines from a PR or comment body.
pub fn clean_body(body: Option<&str>) -> String {
    let body = body.unwrap_or_default().replace("\r\n", "\n");
    let body = HTML_COMMENT.replace_all(&body, "");
    let body = BLANK_LINES.replace_all(&body, "\n\n");
    body.trim().to_string()
}

/// Renders a cleaned body as markdown. The text is copied so it can be kept in the details cache.
pub fn render_markdown(body: &str) -> Text<'static> {
    let text = tui_markdown::from_str(body);
    Text::from(
        text.lines
            .into_iter()
            .map(|line| {
                let spans: Vec<Span<'static>> = line
                    .spans
                    .into_iter()
                    .map(|span| Span::styled(span.content.into_owned(), span.style))
                    .collect();
                Line {
                    spans,
                    ..line
                }
            })
            .collect::<Vec<_>>(),
    )
}

/// Whether `tui-markdown` drew this line as part of a table.
fn is_table_line(line: &Line) -> bool {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .find(|c| !c.is_whitespace())
        .is_some_and(|c| matches!(c, '┌' | '│' | '├' | '└'))
}

/// The width of the widest table line, which is as far as a body can be scrolled sideways.
pub fn widest_table(text: &Text) -> usize {
    text.lines
        .iter()
        .filter(|line| is_table_line(line))
        .map(Line::width)
        .max()
        .unwrap_or(0)
}

/// Shows `width` columns of each table line, starting `x` columns in. Other lines are left to
/// wrap, so a table can be scrolled sideways while the text around it stays put.
pub fn crop_tables(text: &Text<'static>, width: usize, x: usize) -> Text<'static> {
    let lines: Vec<Line<'static>> = text
        .lines
        .iter()
        .map(|line| {
            if !is_table_line(line) {
                return line.clone();
            }
            let mut column = 0;
            let mut spans = Vec::new();
            for span in &line.spans {
                let mut kept = String::new();
                for c in span.content.chars() {
                    let w = c.width().unwrap_or(0);
                    if column >= x && column + w <= x + width {
                        kept.push(c);
                    }
                    column += w;
                }
                if !kept.is_empty() {
                    spans.push(Span::styled(kept, span.style));
                }
            }
            Line {
                spans,
                ..line.clone()
            }
        })
        .collect();
    Text {
        lines,
        ..text.clone()
    }
}

/// Names the title columns can't use, as other parts of a PR already have them.
const RESERVED_COLUMNS: [&str; 13] = [
    "number", "title", "branch", "url", "changes", "body", "comments", "diff", "state", "checks",
    "runs", "review", "updated",
];

/// Compiles the `--column-regex` and checks its named groups can be columns.
pub fn parse_column_regex(pattern: &str) -> Result<Regex, String> {
    let regex = Regex::new(pattern).map_err(|error| error.to_string())?;
    let names = column_names(&regex);
    if names.is_empty() {
        return Err("--column-regex needs at least one named group, e.g. '(?<package>\\S+) to (?<version>\\S+)'".to_string());
    }
    let clashes: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| RESERVED_COLUMNS.contains(name))
        .collect();
    if !clashes.is_empty() {
        return Err(format!(
            "--column-regex group names clash with built-in columns: {}",
            clashes.join(", ")
        ));
    }
    Ok(regex)
}

/// The names of the regex's groups, in order. They are the table's column headers.
pub fn column_names(regex: &Regex) -> Vec<String> {
    regex
        .capture_names()
        .flatten()
        .map(str::to_string)
        .collect()
}

/// What each named group matched in `title`, or empty text when the title doesn't match.
pub fn column_cells(regex: &Regex, title: &str) -> Vec<String> {
    let captures = regex.captures(title);
    column_names(regex)
        .iter()
        .map(|name| {
            captures
                .as_ref()
                .and_then(|captures| captures.name(name))
                .map_or("", |found| found.as_str())
                .to_string()
        })
        .collect()
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
    fn humanises_times() {
        let now: jiff::Timestamp = "2026-03-10T12:00:00Z".parse().unwrap();
        let ago = |timestamp| ago(timestamp, now);
        assert_eq!(ago("2026-03-10T12:00:00Z"), "just now");
        assert_eq!(ago("2026-03-10T11:59:01Z"), "just now");
        assert_eq!(ago("2026-03-10T11:59:00Z"), "1 minute ago");
        assert_eq!(ago("2026-03-10T11:15:00Z"), "45 minutes ago");
        assert_eq!(ago("2026-03-10T09:00:00Z"), "3 hours ago");
        assert_eq!(ago("2026-03-09T12:00:00Z"), "1 day ago");
        assert_eq!(ago("2026-03-01T12:00:00Z"), "1 week ago");
        assert_eq!(ago("2026-01-10T12:00:00Z"), "1 month ago");
        assert_eq!(ago("2024-03-10T12:00:00Z"), "2 years ago");
        // The future, such as a clock that is a little off, isn't an error.
        assert_eq!(ago("2026-03-10T12:00:30Z"), "just now");
        assert_eq!(ago("yesterday"), "yesterday");
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
    fn cropping_moves_only_table_lines() {
        let text = render_markdown("A paragraph of prose.\n\n| Name | Value |\n|---|---|\n| alpha | 123456789 |");
        let widest = widest_table(&text);
        assert!(widest > 10);
        let plain = |text: &Text| text.lines.iter().map(Line::to_string).collect::<Vec<_>>();
        let before = plain(&text);
        let cropped = plain(&crop_tables(&text, 10, 2));
        for (before, cropped) in before.iter().zip(&cropped) {
            if before.starts_with(['┌', '│', '├', '└']) {
                assert_eq!(cropped, &before.chars().skip(2).take(10).collect::<String>());
            } else {
                assert_eq!(cropped, before);
            }
        }
    }

    #[test]
    fn markdown_keeps_details_tags_and_shows_image_alt_text() {
        let text = render_markdown("<details>\n<summary>Notes</summary>\n\n![badge](https://example.com/b.svg)\n\n</details>");
        let plain: Vec<String> = text.lines.iter().map(Line::to_string).collect();
        assert!(plain.contains(&"<summary>Notes</summary>".to_string()), "{plain:?}");
        assert!(plain.iter().any(|l| l.contains("badge") && !l.contains("example.com")), "{plain:?}");
    }

    const BUMP: &str = r"Bump (?<container>.+?)(?: from \S+)? to (?<version>\S+)$";

    #[test]
    fn the_columns_are_the_named_groups_in_order() {
        let regex = parse_column_regex(BUMP).unwrap();
        assert_eq!(column_names(&regex), ["container", "version"]);
        assert_eq!(
            column_cells(&regex, "Bump example-lib from 1.0.0 to 1.2.0"),
            ["example-lib", "1.2.0"]
        );
        assert_eq!(
            column_cells(&regex, "Bump example-lib to 1.2.0"),
            ["example-lib", "1.2.0"]
        );
        // A title that doesn't match, or a group that doesn't take part, gives an empty cell.
        assert_eq!(column_cells(&regex, "Fix a bug"), ["", ""]);
        let optional = parse_column_regex(r"(?<a>x)(?<b>y)?").unwrap();
        assert_eq!(column_cells(&optional, "x"), ["x", ""]);
    }

    #[test]
    fn a_bad_column_regex_is_rejected() {
        assert!(parse_column_regex("(unclosed").is_err());
        let error = parse_column_regex(r"(\S+) to (\S+)").unwrap_err();
        assert!(error.contains("at least one named group"));
        let error = parse_column_regex(r"(?<title>\S+) to (?<version>\S+)").unwrap_err();
        assert_eq!(
            error,
            "--column-regex group names clash with built-in columns: title"
        );
        let error = parse_column_regex(r"(?<checks>\S+) (?<review>\S+)").unwrap_err();
        assert!(error.ends_with("checks, review"));
    }
}
