//! Everything that talks to the `gh` CLI. `gh` handles authentication, so this program never
//! touches tokens.

use std::io::ErrorKind;
use std::process::Stdio;

use color_eyre::eyre::{Result, bail};
use serde::Deserialize;
use tokio::process::Command;

/// Fields requested from `gh pr list --json`.
const PR_FIELDS: &str = "number,title,state,headRefName,files,labels,body,comments,\
    statusCheckRollup,mergeable,reviewDecision,additions,deletions,updatedAt,url";

pub struct Gh {
    repo: Option<String>,
}

impl Gh {
    pub fn new(repo: Option<String>) -> Self {
        Self { repo }
    }

    pub async fn list_prs(&self, query: &str, limit: u32) -> Result<Vec<Pr>> {
        let limit = limit.to_string();
        let args = [
            "pr", "list", "--search", query, "--limit", &limit, "--json", PR_FIELDS,
        ];
        let stdout = self.run(&args).await?;
        Ok(serde_json::from_slice(&stdout)?)
    }

    /// Runs `gh <args> [--repo <repo>]` and returns its stdout. `--repo` belongs to each
    /// subcommand rather than to `gh` itself, so it goes last.
    async fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        let mut args: Vec<&str> = args.to_vec();
        if let Some(repo) = &self.repo {
            args.extend(["--repo", repo]);
        }
        let output = Command::new("gh")
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .await?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = [stdout.trim(), stderr.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            bail!("`{}` failed:\n{message}", render_command(&args));
        }
        Ok(output.stdout)
    }
}

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

// Most fields are first read by later steps (checks, panes, actions).
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub state: PrState,
    pub head_ref_name: String,
    pub files: Vec<File>,
    pub labels: Vec<Label>,
    pub body: Option<String>,
    pub comments: Vec<Comment>,
    pub status_check_rollup: Vec<Check>,
    pub mergeable: String,
    pub review_decision: String,
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

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub author: Option<Author>,
    pub created_at: String,
    pub body: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Author {
    pub login: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
pub enum Check {
    CheckRun(CheckRun),
    StatusContext(StatusContext),
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRun {
    pub name: String,
    pub workflow_name: String,
    pub status: String,
    pub conclusion: String,
    pub started_at: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusContext {
    pub context: String,
    pub state: String,
    pub started_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
      {
        "number": 12,
        "title": "ABC-1: Update example-lib to v2",
        "state": "OPEN",
        "headRefName": "renovate/example-lib-2.x",
        "files": [{"path": "Cargo.toml", "additions": 1, "deletions": 1}],
        "labels": [{"id": "L1", "name": "deps", "description": "", "color": "ededed"}],
        "body": null,
        "comments": [
          {"author": null, "createdAt": "2026-01-02T03:04:05Z", "body": "hello"},
          {"author": {"login": "someone"}, "createdAt": "2026-01-02T03:04:06Z", "body": null}
        ],
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
        "additions": 1,
        "deletions": 1,
        "updatedAt": "2026-01-02T03:04:06Z",
        "url": "https://github.com/example/repo/pull/12"
      },
      {
        "number": 13,
        "title": "Merged one",
        "state": "MERGED",
        "headRefName": "b",
        "files": [],
        "labels": [],
        "body": "text",
        "comments": [],
        "statusCheckRollup": [],
        "mergeable": "UNKNOWN",
        "reviewDecision": "APPROVED",
        "additions": 0,
        "deletions": 0,
        "updatedAt": "2026-01-02T03:04:06Z",
        "url": "https://github.com/example/repo/pull/13"
      }
    ]"#;

    #[test]
    fn parses_pr_list() {
        let prs: Vec<Pr> = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(prs.len(), 2);

        let pr = &prs[0];
        assert_eq!(pr.state, PrState::Open);
        assert!(pr.body.is_none());
        assert!(pr.comments[0].author.is_none());
        assert!(pr.comments[1].body.is_none());
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
