//! Everything that talks to the `gh` CLI. `gh` handles authentication, so this program never
//! touches tokens.

use std::fmt;
use std::io::ErrorKind;
use std::process::Stdio;
use std::time::Duration;

use color_eyre::eyre::{Result, bail, eyre};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::process::Command;

use crate::cli::MergeMethod;

/// Fields requested from `gh pr list --json`: only what the table needs, so it renders sooner.
const LIST_FIELDS: &str = "number,title,state,statusCheckRollup,mergeable,reviewDecision,updatedAt";

/// Fields requested by the timed refresh: what can change under a row that is already listed.
const STATUS_FIELDS: &str = "number,state,statusCheckRollup,mergeable,reviewDecision,updatedAt";

/// The most PRs a refresh looks up one by one because the list no longer returns them.
const MAX_MISSING_LOOKUPS: usize = 10;

/// Fields requested from `gh pr view --json` for the highlighted PR.
const DETAIL_FIELDS: &str = "headRefName,body,comments,additions,deletions";

#[derive(Clone)]
pub struct Gh {
    repo: Option<String>,
    /// Whether actions skip `gh`: see [`Gh::run_action`].
    dry_run: bool,
}

impl Gh {
    pub fn new(repo: Option<String>, dry_run: bool) -> Self {
        Self { repo, dry_run }
    }

    /// Lists up to one more PR than `limit`, so the caller can tell whether the list was cut
    /// short.
    pub async fn list_prs(&self, query: &str, limit: u32) -> Result<Vec<Pr>> {
        let stdout = self.list_json(query, limit, LIST_FIELDS).await?;
        Ok(serde_json::from_slice(&stdout)?)
    }

    /// The status of each PR the query lists, and of up to [`MAX_MISSING_LOOKUPS`] of the
    /// `candidates` it doesn't, such as a PR merged by someone else since the list loaded. It
    /// uses what it got even if some of it failed, and says why in `failure`.
    pub async fn status_update(
        &self,
        query: &str,
        limit: u32,
        candidates: Vec<u64>,
    ) -> StatusUpdate {
        let mut update = StatusUpdate::default();
        match self.list_statuses(query, limit).await {
            Ok(statuses) => update.statuses = statuses,
            Err(e) => {
                update.failure = Some(e.to_string());
                return update;
            }
        }
        let missing: Vec<u64> = candidates
            .into_iter()
            .filter(|number| {
                !update
                    .statuses
                    .iter()
                    .any(|status| status.number == *number)
            })
            .take(MAX_MISSING_LOOKUPS)
            .collect();
        let lookups = futures::future::join_all(missing.into_iter().map(|n| self.pr_status(n)));
        for result in lookups.await {
            match result {
                Ok(status) => update.statuses.push(status),
                Err(e) => update.failure = update.failure.take().or(Some(e.to_string())),
            }
        }
        update
    }

    async fn list_statuses(&self, query: &str, limit: u32) -> Result<Vec<PrStatus>> {
        let stdout = self.list_json(query, limit, STATUS_FIELDS).await?;
        Ok(serde_json::from_slice(&stdout)?)
    }

    async fn pr_status(&self, number: u64) -> Result<PrStatus> {
        self.view_json(number, STATUS_FIELDS).await
    }

    /// `gh pr view` for one PR with these `--json` fields.
    async fn view_json<T: DeserializeOwned>(&self, number: u64, fields: &str) -> Result<T> {
        let number = number.to_string();
        let stdout = self.run(&["pr", "view", &number, "--json", fields]).await?;
        Ok(serde_json::from_slice(&stdout)?)
    }

    /// `gh pr list` for the query, asking for one more PR than `limit`.
    async fn list_json(&self, query: &str, limit: u32, fields: &str) -> Result<Vec<u8>> {
        let limit = limit.saturating_add(1).to_string();
        // Without `--state all`, gh adds `state:open` when the query has no state qualifier. A
        // qualifier in the query still wins, as on GitHub's web UI.
        // The query is joined to the flag with `=`, so one that starts with `-`, such as
        // `-label:major`, can't be read as a flag of its own.
        let search = format!("--search={query}");
        let args = [
            "pr", "list", "--state", "all", &search, "--limit", &limit, "--json", fields,
        ];
        self.run(&args).await
    }

    /// One PR with the same fields as the list, to refresh its row.
    pub async fn pr(&self, number: u64) -> Result<Pr> {
        self.view_json(number, LIST_FIELDS).await
    }

    /// Opens the PR in the browser. It only reads, so it also runs under `--dry-run`.
    pub async fn open(&self, number: u64) -> Result<()> {
        let number = number.to_string();
        self.run(&["pr", "view", &number, "--web"]).await?;
        Ok(())
    }

    pub async fn approve(&self, number: u64) -> Result<()> {
        let number = number.to_string();
        self.run_action(&["pr", "review", &number, "--approve"])
            .await
    }

    /// Merges a PR. With `auto`, GitHub merges it once its checks pass; otherwise it is merged
    /// now and its branch deleted. The empty body keeps a squash commit's message body empty.
    pub async fn merge(&self, number: u64, auto: bool, method: MergeMethod) -> Result<()> {
        let number = number.to_string();
        let mut args = vec!["pr", "merge", &number];
        if auto {
            args.extend(["--auto", method.flag()]);
        } else {
            args.extend([method.flag(), "--delete-branch"]);
        }
        args.extend(["--body", ""]);
        self.run_action(&args).await
    }

    /// Runs a `gh` command that acts on one PR. If it fails, the error is gh's output alone:
    /// the caller already knows which PR it was. Every command that changes a PR goes through
    /// here, so a dry run makes no `gh` call: it waits a moment and succeeds.
    async fn run_action(&self, args: &[&str]) -> Result<()> {
        if self.dry_run {
            tokio::time::sleep(Duration::from_millis(300)).await;
            return Ok(());
        }
        match self.exec(args, false).await {
            Ok(_) => Ok(()),
            Err(e) => match e.downcast::<GhError>() {
                Ok(failure) => Err(eyre!(failure.output)),
                Err(e) => Err(e),
            },
        }
    }

    pub async fn pr_details(&self, number: u64) -> Result<PrDetails> {
        self.view_json(number, DETAIL_FIELDS).await
    }

    /// The PR's diff, with the ANSI colour codes gh adds. Without `--color=always` gh leaves
    /// them out, as its output isn't a terminal.
    pub async fn pr_diff(&self, number: u64) -> Result<String> {
        let number = number.to_string();
        let stdout = self.run(&["pr", "diff", &number, "--color=always"]).await?;
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    }

    /// Runs a read-only `gh` command and returns its stdout. If the caller stops waiting, such as
    /// when the program quits, `gh` is killed rather than left running.
    async fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        self.exec(args, true).await
    }

    /// Runs `gh <args> [--repo <repo>]` and returns its stdout. `--repo` belongs to each
    /// subcommand rather than to `gh` itself, so it goes last. With `kill_on_drop`, `gh` is
    /// killed if the caller stops waiting. Actions leave it off: stopping one halfway could leave
    /// a PR approved but not merged, or the reverse, with nothing to show for it.
    async fn exec(&self, args: &[&str], kill_on_drop: bool) -> Result<Vec<u8>> {
        let mut args: Vec<&str> = args.to_vec();
        if let Some(repo) = &self.repo {
            args.extend(["--repo", repo]);
        }
        let output = Command::new("gh")
            .args(&args)
            .stdin(Stdio::null())
            .kill_on_drop(kill_on_drop)
            .output()
            .await?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut message = [stdout.trim(), stderr.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if message.is_empty() {
                message = format!("exited with {}", output.status);
            }
            return Err(GhError {
                command: render_command(&args),
                output: message,
            }
            .into());
        }
        Ok(output.stdout)
    }
}

/// A `gh` command that exited with an error.
#[derive(Debug)]
pub struct GhError {
    /// The command, ready to copy into a shell.
    pub command: String,
    /// Its stdout and stderr joined, or its exit status if both are empty.
    pub output: String,
}

impl fmt::Display for GhError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "`{}` failed:\n{}", self.command, self.output)
    }
}

impl std::error::Error for GhError {}

pub fn check_installed() -> Result<()> {
    match std::process::Command::new("gh")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => bail!("gh is not installed."),
        Err(e) => Err(e.into()),
    }
}

/// Renders a `gh` command so it can be copied into a shell and run again.
fn render_command(args: &[&str]) -> String {
    let mut line = String::from("gh");
    for arg in args {
        line.push(' ');
        if !arg.is_empty()
            && arg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:,=@".contains(c))
        {
            line.push_str(arg);
        } else {
            line.push('\'');
            line.push_str(&arg.replace('\'', r"'\''"));
            line.push('\'');
        }
    }
    line
}

/// A PR as listed in the table.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub state: PrState,
    pub status_check_rollup: Vec<Check>,
    pub mergeable: Mergeable,
    pub review_decision: ReviewDecision,
    /// When the PR last changed, as GitHub reports it. A change means its details and diff may
    /// have changed too.
    pub updated_at: String,
}

impl Pr {
    /// Takes the fields a refresh brings, leaving the title.
    pub fn update(&mut self, status: PrStatus) {
        self.state = status.state;
        self.status_check_rollup = status.status_check_rollup;
        self.mergeable = status.mergeable;
        self.review_decision = status.review_decision;
        self.updated_at = status.updated_at;
    }
}

/// What a refresh brings for a PR: the fields of [`Pr`] that change.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrStatus {
    pub number: u64,
    pub state: PrState,
    pub status_check_rollup: Vec<Check>,
    pub mergeable: Mergeable,
    pub review_decision: ReviewDecision,
    pub updated_at: String,
}

/// The result of a timed refresh.
#[derive(Debug, Default)]
pub struct StatusUpdate {
    pub statuses: Vec<PrStatus>,
    /// Why part or all of the refresh failed, if it did.
    pub failure: Option<String>,
}

/// Whether a PR merges cleanly. Only a conflict matters here.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Mergeable {
    Conflicting,
    #[serde(other)]
    Other,
}

/// GitHub's review decision. It is an empty string when no review is required.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    /// Review required, or no decision.
    #[serde(other)]
    Other,
}

/// The status of a check run. Anything but `Completed` is still going.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStatus {
    Completed,
    #[serde(other)]
    Other,
}

/// How a completed check run ended. Anything but the first three counts as a failure.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Conclusion {
    Success,
    Neutral,
    Skipped,
    #[serde(other)]
    Other,
}

/// The state of a commit status. Anything but the first three counts as a failure.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContextState {
    Success,
    Pending,
    Expected,
    #[serde(other)]
    Other,
}

/// The rest of a PR, fetched only when it is highlighted.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDetails {
    pub head_ref_name: String,
    pub body: Option<String>,
    pub comments: Vec<Comment>,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub author: Option<Author>,
    pub created_at: String,
    pub body: Option<String>,
    /// The comment's page in the web UI.
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Author {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "__typename")]
pub enum Check {
    CheckRun(CheckRun),
    StatusContext(StatusContext),
    /// Any other kind. It is kept, rather than failing the whole list to parse, and counts as
    /// pending: it can't be shown to have passed.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRun {
    pub name: String,
    pub workflow_name: String,
    pub status: RunStatus,
    pub conclusion: Conclusion,
    pub started_at: Option<String>,
    /// The job's page.
    pub details_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusContext {
    pub context: String,
    pub state: ContextState,
    pub started_at: Option<String>,
    /// The page of the build, which can be on another system, such as TeamCity.
    pub target_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_FIXTURE: &str = r#"[
      {
        "number": 12,
        "title": "ABC-1: Update example-lib to v2",
        "state": "OPEN",
        "statusCheckRollup": [
          {
            "__typename": "CheckRun",
            "name": "build",
            "workflowName": "CI",
            "status": "COMPLETED",
            "conclusion": "SUCCESS",
            "startedAt": "2026-01-02T03:00:00Z"
          },
          {"__typename": "StatusContext", "context": "ci/legacy", "state": "PENDING"}
        ],
        "mergeable": "MERGEABLE",
        "reviewDecision": "",
        "updatedAt": "2026-01-02T03:04:06Z"
      },
      {
        "number": 13,
        "title": "Merged one",
        "state": "MERGED",
        "statusCheckRollup": [],
        "mergeable": "UNKNOWN",
        "reviewDecision": "APPROVED",
        "updatedAt": "2026-01-02T03:04:07Z"
      }
    ]"#;

    const DETAIL_FIXTURE: &str = r#"{
      "headRefName": "renovate/example-lib-2.x",
      "body": null,
      "comments": [
        {"author": null, "createdAt": "2026-01-02T03:04:05Z", "body": "hello"},
        {"author": {"login": "someone"}, "createdAt": "2026-01-02T03:04:06Z", "body": null}
      ],
      "additions": 1,
      "deletions": 1
    }"#;

    #[test]
    fn parses_pr_list() {
        let prs: Vec<Pr> = serde_json::from_str(LIST_FIXTURE).unwrap();
        assert_eq!(prs.len(), 2);

        let pr = &prs[0];
        assert_eq!(pr.state, PrState::Open);
        assert!(matches!(
            &pr.status_check_rollup[0],
            Check::CheckRun(run) if run.workflow_name == "CI" && run.started_at.is_some()
        ));
        assert!(matches!(
            &pr.status_check_rollup[1],
            Check::StatusContext(ctx) if ctx.state == ContextState::Pending && ctx.started_at.is_none()
        ));

        assert_eq!(prs[1].state, PrState::Merged);
    }

    #[test]
    fn an_unknown_kind_of_check_does_not_fail_the_list() {
        let json = r#"[{
          "number": 1,
          "title": "t",
          "state": "OPEN",
          "statusCheckRollup": [
            {"__typename": "SomethingNew", "foo": "bar"},
            {"__typename": "StatusContext", "context": "ci/legacy", "state": "SUCCESS"}
          ],
          "mergeable": "MERGEABLE",
          "reviewDecision": "",
          "updatedAt": "2026-01-02T03:04:06Z"
        }]"#;
        let prs: Vec<Pr> = serde_json::from_str(json).unwrap();
        assert!(matches!(prs[0].status_check_rollup[0], Check::Unknown));
        assert!(matches!(
            prs[0].status_check_rollup[1],
            Check::StatusContext(_)
        ));
    }

    #[test]
    fn parses_a_status_refresh_and_updates_a_pr() {
        let json = r#"[{
          "number": 12,
          "state": "MERGED",
          "statusCheckRollup": [],
          "mergeable": "UNKNOWN",
          "reviewDecision": "APPROVED",
          "updatedAt": "2026-02-03T04:05:06Z"
        }]"#;
        let statuses: Vec<PrStatus> = serde_json::from_str(json).unwrap();
        let mut prs: Vec<Pr> = serde_json::from_str(LIST_FIXTURE).unwrap();
        let mut pr = prs.remove(0);
        pr.update(statuses.into_iter().next().unwrap());
        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(pr.review_decision, ReviewDecision::Approved);
        assert_eq!(pr.updated_at, "2026-02-03T04:05:06Z");
        assert!(pr.status_check_rollup.is_empty());
        // The title isn't part of a refresh.
        assert_eq!(pr.title, "ABC-1: Update example-lib to v2");
    }

    #[test]
    fn parses_pr_details() {
        let details: PrDetails = serde_json::from_str(DETAIL_FIXTURE).unwrap();
        assert!(details.body.is_none());
        assert!(details.comments[0].author.is_none());
        assert!(details.comments[1].body.is_none());
    }

    #[test]
    fn renders_command_for_a_shell() {
        assert_eq!(
            render_command(&[
                "pr",
                "list",
                "--search",
                "is:pr author:app/renovate",
                "--repo",
                "o/r"
            ]),
            "gh pr list --search 'is:pr author:app/renovate' --repo o/r"
        );
        assert_eq!(render_command(&["it's", ""]), r"gh 'it'\''s' ''");
    }
}
