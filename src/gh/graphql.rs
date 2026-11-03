//! The reads, as GraphQL queries sent with `gh api graphql`: the queries, the raw shape GitHub
//! answers in, and what turns that into the types the rest of the program uses. `gh pr list` did
//! several of those conversions for us, so they are done here and tested.

use serde::Deserialize;
use serde::de::IgnoredAny;

use super::{
    Author, Check, CheckRun, Conclusion, ContextState, Mergeable, Pr, PrState, ReviewDecision,
    RunStatus, StatusContext,
};

/// The most nodes GitHub gives for one `first:`.
pub const MAX_PAGE: u32 = 100;

/// The qualifiers every search starts with. `gh pr list` adds both: without `repo:` the search is
/// of all of GitHub, and without `is:pr` it finds issues too. `{owner}` and `{repo}` are filled in
/// by `gh` from the repository (`GH_REPO`, or the current directory's), in the values passed with
/// `-F`, so the query is passed that way.
pub const SEARCH_PREFIX: &str = "repo:{owner}/{repo} is:pr";

/// The PRs a query finds, `$first` at a time. The query is only sent in its variables, so what a
/// user types can't change the shape of the query.
pub const LIST_QUERY: &str = r#"
query($q: String!, $first: Int!, $after: String) {
  search(query: $q, type: ISSUE_ADVANCED, first: $first, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      ... on PullRequest {
        number
        title
        state
        mergeable
        reviewDecision
        updatedAt
        url
        author { __typename login }
        commits(last: 1) {
          nodes {
            commit {
              statusCheckRollup {
                contexts(first: 100) {
                  nodes {
                    __typename
                    ... on CheckRun {
                      name
                      status
                      conclusion
                      startedAt
                      detailsUrl
                      checkSuite { workflowRun { workflow { name } } }
                    }
                    ... on StatusContext { context state createdAt targetUrl }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#;

/// The search string for a user's `query`.
pub fn search_string(query: &str) -> String {
    format!("{SEARCH_PREFIX} {query}")
}

#[derive(Debug, Deserialize)]
pub struct Response<T> {
    pub data: T,
}

#[derive(Debug, Deserialize)]
pub struct SearchData {
    pub search: Search,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Search {
    pub page_info: PageInfo,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

/// A search result. Only PRs are asked for fields, so anything else comes back as `{}`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Node {
    Pr(Box<GqlPr>),
    Other(IgnoredAny),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlPr {
    number: u64,
    title: String,
    state: PrState,
    mergeable: Mergeable,
    /// `null` when no review is required, where `gh` gave `""`.
    review_decision: Option<ReviewDecision>,
    updated_at: String,
    url: String,
    /// `null` for an account that has been deleted.
    author: Option<GqlActor>,
    commits: Commits,
}

#[derive(Debug, Deserialize)]
struct GqlActor {
    #[serde(rename = "__typename")]
    kind: String,
    login: String,
}

#[derive(Debug, Deserialize)]
struct Commits {
    nodes: Vec<CommitNode>,
}

#[derive(Debug, Deserialize)]
struct CommitNode {
    commit: Commit,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    /// `null` for a commit with no checks.
    status_check_rollup: Option<Rollup>,
}

#[derive(Debug, Deserialize)]
struct Rollup {
    contexts: Contexts,
}

#[derive(Debug, Deserialize)]
struct Contexts {
    nodes: Vec<GqlCheck>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
enum GqlCheck {
    CheckRun(GqlCheckRun),
    StatusContext(GqlStatusContext),
    /// Any other kind: see [`Check::Unknown`].
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlCheckRun {
    name: String,
    status: RunStatus,
    /// `null` until the run completes, where `gh` gave `""`.
    conclusion: Option<Conclusion>,
    started_at: Option<String>,
    details_url: Option<String>,
    check_suite: Option<CheckSuite>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckSuite {
    workflow_run: Option<WorkflowRun>,
}

#[derive(Debug, Deserialize)]
struct WorkflowRun {
    workflow: Workflow,
}

#[derive(Debug, Deserialize)]
struct Workflow {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlStatusContext {
    context: String,
    state: ContextState,
    /// A status has no start; `gh` gave its creation time as `startedAt`.
    created_at: Option<String>,
    target_url: Option<String>,
}

/// The name `gh` gave an account that has been deleted, and GitHub's web UI does too.
const DELETED_ACCOUNT: &str = "ghost";

/// A login as `gh` gave it, and as `author:` takes it in a query: a bot's has the `app/` prefix.
fn login(actor: Option<GqlActor>) -> String {
    match actor {
        Some(GqlActor { kind, login }) if kind == "Bot" => format!("app/{login}"),
        Some(GqlActor { login, .. }) => login,
        None => DELETED_ACCOUNT.to_string(),
    }
}

impl From<GqlCheck> for Check {
    fn from(check: GqlCheck) -> Self {
        match check {
            GqlCheck::CheckRun(run) => Check::CheckRun(CheckRun {
                name: run.name,
                workflow_name: run
                    .check_suite
                    .and_then(|suite| suite.workflow_run)
                    .map(|run| run.workflow.name)
                    .unwrap_or_default(),
                status: run.status,
                conclusion: run.conclusion.unwrap_or(Conclusion::Other),
                started_at: run.started_at,
                details_url: run.details_url,
            }),
            GqlCheck::StatusContext(status) => Check::StatusContext(StatusContext {
                context: status.context,
                state: status.state,
                started_at: status.created_at,
                target_url: status.target_url,
            }),
            GqlCheck::Unknown => Check::Unknown,
        }
    }
}

impl From<GqlPr> for Pr {
    fn from(pr: GqlPr) -> Self {
        let checks = pr
            .commits
            .nodes
            .into_iter()
            .next()
            .and_then(|node| node.commit.status_check_rollup)
            .map(|rollup| rollup.contexts.nodes)
            .unwrap_or_default();
        Pr {
            number: pr.number,
            title: pr.title,
            state: pr.state,
            status_check_rollup: checks.into_iter().map(Check::from).collect(),
            mergeable: pr.mergeable,
            review_decision: pr.review_decision.unwrap_or(ReviewDecision::Other),
            url: pr.url,
            updated_at: pr.updated_at,
            author: Author {
                login: login(pr.author),
            },
        }
    }
}

impl Search {
    /// The PRs of this page, leaving out anything that isn't one.
    pub fn prs(self) -> impl Iterator<Item = Pr> {
        self.nodes.into_iter().filter_map(|node| match node {
            Node::Pr(pr) => Some(Pr::from(*pr)),
            Node::Other(_) => None,
        })
    }
}

/// What a failed `gh api graphql` said, made short. For an error in the query or in what it
/// asked for, `gh` prints the whole response, JSON and all, so this keeps its `errors[].message`s.
/// Anything else, such as a failed login or no network, is already a message and is left alone.
pub fn error_message(output: &str) -> String {
    #[derive(Deserialize)]
    struct Failed {
        errors: Vec<Reason>,
    }
    #[derive(Deserialize)]
    struct Reason {
        message: String,
    }
    // The response is on the first line; `gh` follows it with a line of its own.
    let first = output.lines().next().unwrap_or_default();
    match serde_json::from_str::<Failed>(first) {
        Ok(failed) if !failed.errors.is_empty() => failed
            .errors
            .iter()
            .map(|reason| reason.message.as_str())
            .collect::<Vec<_>>()
            .join("; "),
        _ => output.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page of a search: a PR with both kinds of check and a bot author, a PR with no checks
    /// and no review decision, a PR whose author was deleted, and something that is not a PR.
    const PAGE: &str = r#"{
      "data": {"search": {
        "pageInfo": {"hasNextPage": true, "endCursor": "abc"},
        "nodes": [
          {
            "number": 12,
            "title": "Update example-lib to v2",
            "state": "OPEN",
            "mergeable": "MERGEABLE",
            "reviewDecision": "REVIEW_REQUIRED",
            "updatedAt": "2026-01-02T03:04:06Z",
            "url": "https://example.test/pull/12",
            "author": {"__typename": "Bot", "login": "renovate"},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": [
              {
                "__typename": "CheckRun",
                "name": "build",
                "status": "COMPLETED",
                "conclusion": "SUCCESS",
                "startedAt": "2026-01-02T03:00:00Z",
                "detailsUrl": "https://example.test/runs/1",
                "checkSuite": {"workflowRun": {"workflow": {"name": "CI"}}}
              },
              {
                "__typename": "CheckRun",
                "name": "deploy",
                "status": "IN_PROGRESS",
                "conclusion": null,
                "startedAt": null,
                "detailsUrl": null,
                "checkSuite": {"workflowRun": null}
              },
              {
                "__typename": "StatusContext",
                "context": "ci/legacy",
                "state": "PENDING",
                "createdAt": "2026-01-02T03:01:00Z",
                "targetUrl": "https://example.test/build/9"
              },
              {"__typename": "SomethingNew", "foo": "bar"}
            ]}}}}]}
          },
          {
            "number": 13,
            "title": "Merged one",
            "state": "MERGED",
            "mergeable": "UNKNOWN",
            "reviewDecision": null,
            "updatedAt": "2026-01-02T03:04:07Z",
            "url": "https://example.test/pull/13",
            "author": {"__typename": "User", "login": "alice"},
            "commits": {"nodes": [{"commit": {"statusCheckRollup": null}}]}
          },
          {
            "number": 14,
            "title": "Orphan",
            "state": "CLOSED",
            "mergeable": "CONFLICTING",
            "reviewDecision": "APPROVED",
            "updatedAt": "2026-01-02T03:04:08Z",
            "url": "https://example.test/pull/14",
            "author": null,
            "commits": {"nodes": []}
          },
          {}
        ]
      }}
    }"#;

    fn page() -> Search {
        serde_json::from_str::<Response<SearchData>>(PAGE)
            .unwrap()
            .data
            .search
    }

    #[test]
    fn a_page_gives_its_cursor_and_leaves_out_what_is_not_a_pr() {
        let search = page();
        assert!(search.page_info.has_next_page);
        assert_eq!(search.page_info.end_cursor.as_deref(), Some("abc"));
        let numbers: Vec<u64> = search.prs().map(|pr| pr.number).collect();
        assert_eq!(numbers, [12, 13, 14]);
    }

    #[test]
    fn a_bot_login_has_the_app_prefix_and_a_deleted_account_is_ghost() {
        let prs: Vec<Pr> = page().prs().collect();
        assert_eq!(prs[0].author.login, "app/renovate");
        assert_eq!(prs[1].author.login, "alice");
        assert_eq!(prs[2].author.login, "ghost");
    }

    #[test]
    fn checks_get_the_workflow_name_and_the_status_start_that_gh_gave() {
        let prs: Vec<Pr> = page().prs().collect();
        let checks = &prs[0].status_check_rollup;
        assert_eq!(checks.len(), 4);
        assert!(matches!(
            &checks[0],
            Check::CheckRun(run) if run.workflow_name == "CI"
                && run.conclusion == Conclusion::Success
                && run.details_url.as_deref() == Some("https://example.test/runs/1")
        ));
        // No workflow, no conclusion yet, and no start.
        assert!(matches!(
            &checks[1],
            Check::CheckRun(run) if run.workflow_name.is_empty()
                && run.status == RunStatus::Other
                && run.conclusion == Conclusion::Other
                && run.started_at.is_none()
        ));
        assert!(matches!(
            &checks[2],
            Check::StatusContext(status) if status.state == ContextState::Pending
                && status.started_at.as_deref() == Some("2026-01-02T03:01:00Z")
                && status.target_url.as_deref() == Some("https://example.test/build/9")
        ));
        assert!(matches!(checks[3], Check::Unknown));
    }

    #[test]
    fn no_checks_or_no_commit_means_an_empty_rollup() {
        let prs: Vec<Pr> = page().prs().collect();
        assert!(prs[1].status_check_rollup.is_empty());
        assert!(prs[2].status_check_rollup.is_empty());
    }

    #[test]
    fn a_missing_review_decision_is_what_an_empty_string_was() {
        let prs: Vec<Pr> = page().prs().collect();
        assert_eq!(prs[0].review_decision, ReviewDecision::Other);
        assert_eq!(prs[1].review_decision, ReviewDecision::Other);
        assert_eq!(prs[2].review_decision, ReviewDecision::Approved);
        assert_eq!(prs[2].mergeable, Mergeable::Conflicting);
        assert_eq!(prs[1].state, PrState::Merged);
    }

    #[test]
    fn the_search_is_of_this_repository_and_of_pull_requests() {
        assert_eq!(
            search_string("state:open author:app/renovate"),
            "repo:{owner}/{repo} is:pr state:open author:app/renovate"
        );
    }

    #[test]
    fn a_graphql_error_is_shown_as_its_messages() {
        let output = concat!(
            r#"{"data":null,"errors":[{"message":"Field 'x' doesn't exist"},{"message":"Second"}]}"#,
            "\ngh: Field 'x' doesn't exist\n"
        );
        assert_eq!(error_message(output), "Field 'x' doesn't exist; Second");
    }

    #[test]
    fn any_other_failure_is_left_as_it_was() {
        let output = "gh: Bad credentials (HTTP 401)";
        assert_eq!(error_message(output), output);
        // JSON with no errors in it is not an error response.
        assert_eq!(error_message(r#"{"data":null}"#), r#"{"data":null}"#);
        assert_eq!(error_message(""), "");
    }
}
