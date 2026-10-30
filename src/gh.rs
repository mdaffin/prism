//! Everything that talks to the `gh` CLI. `gh` handles authentication, so this program never
//! touches tokens.

use std::fmt;
use std::io::ErrorKind;
use std::process::Stdio;
use std::time::Duration;

use color_eyre::eyre::{Result, bail, eyre};
use serde::Deserialize;
use tokio::process::Command;

use crate::cli::MergeMethod;

/// Fields requested from `gh pr list --json`: only what the table needs, so it renders sooner.
const LIST_FIELDS: &str = "number,title,state,statusCheckRollup,mergeable,reviewDecision";

/// Fields requested from `gh pr view --json` for the highlighted PR.
const DETAIL_FIELDS: &str =
    "headRefName,files,labels,body,comments,additions,deletions,updatedAt,url";

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

    pub async fn list_prs(&self, query: &str, limit: u32) -> Result<Vec<Pr>> {
        let limit = limit.to_string();
        // Without `--state all`, gh adds `state:open` when the query has no state qualifier. A
        // qualifier in the query still wins, as on GitHub's web UI.
        // The query is joined to the flag with `=`, so one that starts with `-`, such as
        // `-label:major`, can't be read as a flag of its own.
        let search = format!("--search={query}");
        let args = [
            "pr",
            "list",
            "--state",
            "all",
            &search,
            "--limit",
            &limit,
            "--json",
            LIST_FIELDS,
        ];
        let stdout = self.run(&args).await?;
        Ok(serde_json::from_slice(&stdout)?)
    }

    /// One PR with the same fields as the list, to refresh its row.
    pub async fn pr(&self, number: u64) -> Result<Pr> {
        let number = number.to_string();
        let args = ["pr", "view", &number, "--json", LIST_FIELDS];
        let stdout = self.run(&args).await?;
        Ok(serde_json::from_slice(&stdout)?)
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
        let args = if auto {
            [
                "pr",
                "merge",
                &number,
                "--auto",
                method.flag(),
                "--body",
                "",
            ]
        } else {
            [
                "pr",
                "merge",
                &number,
                method.flag(),
                "--delete-branch",
                "--body",
                "",
            ]
        };
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
        let number = number.to_string();
        let args = ["pr", "view", &number, "--json", DETAIL_FIELDS];
        let stdout = self.run(&args).await?;
        Ok(serde_json::from_slice(&stdout)?)
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
    pub mergeable: String,
    pub review_decision: String,
}

/// The rest of a PR, fetched only when it is highlighted.
// Most fields are first read by later steps (panes, Ctrl-O).
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDetails {
    pub head_ref_name: String,
    pub files: Vec<File>,
    pub labels: Vec<Label>,
    pub body: Option<String>,
    pub comments: Vec<Comment>,
    pub additions: u64,
    pub deletions: u64,
    pub updated_at: String,
    pub url: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct File {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub author: Option<Author>,
    pub created_at: String,
    pub body: Option<String>,
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
    pub status: String,
    pub conclusion: String,
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusContext {
    pub context: String,
    pub state: String,
    pub started_at: Option<String>,
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
        "reviewDecision": ""
      },
      {
        "number": 13,
        "title": "Merged one",
        "state": "MERGED",
        "statusCheckRollup": [],
        "mergeable": "UNKNOWN",
        "reviewDecision": "APPROVED"
      }
    ]"#;

    const DETAIL_FIXTURE: &str = r#"{
      "headRefName": "renovate/example-lib-2.x",
      "files": [{"path": "Cargo.toml", "additions": 1, "deletions": 1}],
      "labels": [{"id": "L1", "name": "deps", "description": "", "color": "ededed"}],
      "body": null,
      "comments": [
        {"author": null, "createdAt": "2026-01-02T03:04:05Z", "body": "hello"},
        {"author": {"login": "someone"}, "createdAt": "2026-01-02T03:04:06Z", "body": null}
      ],
      "additions": 1,
      "deletions": 1,
      "updatedAt": "2026-01-02T03:04:06Z",
      "url": "https://github.com/example/repo/pull/12"
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
            Check::StatusContext(ctx) if ctx.state == "PENDING" && ctx.started_at.is_none()
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
          "reviewDecision": ""
        }]"#;
        let prs: Vec<Pr> = serde_json::from_str(json).unwrap();
        assert!(matches!(prs[0].status_check_rollup[0], Check::Unknown));
        assert!(matches!(
            prs[0].status_check_rollup[1],
            Check::StatusContext(_)
        ));
    }

    #[test]
    fn parses_pr_details() {
        let details: PrDetails = serde_json::from_str(DETAIL_FIXTURE).unwrap();
        assert!(details.body.is_none());
        assert!(details.comments[0].author.is_none());
        assert!(details.comments[1].body.is_none());
        assert_eq!(details.labels[0].name, "deps");
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
