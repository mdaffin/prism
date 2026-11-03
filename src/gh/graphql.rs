//! The reads, as GraphQL queries sent with `gh api graphql`: the queries, the raw shape GitHub
//! answers in, and what turns that into the types the rest of the program uses. `gh pr list` did
//! several of those conversions for us, so they are done here and tested.

use std::collections::HashMap;

use serde::Deserialize;
use serde::de::IgnoredAny;

use super::{
    Author, Check, CheckRun, Comment, Conclusion, ContextState, Mergeable, Pr, PrDetails, PrState,
    PrStatus, ReviewDecision, RunStatus, StatusContext,
};

/// The most nodes GitHub gives for one `first:`.
pub const MAX_PAGE: u32 = 100;

/// The qualifiers every search starts with. `gh pr list` adds both: without `repo:` the search is
/// of all of GitHub, and without `is:pr` it finds issues too. `{owner}` and `{repo}` are filled in
/// by `gh` from the repository (`GH_REPO`, or the current directory's), in the values passed with
/// `-F`, so the query is passed that way.
pub const SEARCH_PREFIX: &str = "repo:{owner}/{repo} is:pr";

/// What changes under a row that is already listed, and so what the timed refresh asks for. Its
/// checks are those of the last commit, as on the PR's page.
const STATUS_FRAGMENT: &str = r#"
fragment StatusFields on PullRequest {
  number
  state
  mergeable
  reviewDecision
  updatedAt
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
"#;

/// What the table lists: the status, and what doesn't change.
const LIST_FRAGMENT: &str = r#"
fragment ListFields on PullRequest {
  ...StatusFields
  title
  url
  author { __typename login }
}
"#;

/// Which of the two sets of fields a query asks for.
#[derive(Clone, Copy)]
pub enum Fields {
    /// [`Status`]: just the status.
    Status,
    /// [`GqlPr`]: everything the table lists.
    List,
}

impl Fields {
    fn name(self) -> &'static str {
        match self {
            Self::Status => "StatusFields",
            Self::List => "ListFields",
        }
    }

    /// The fragments that the fields are made of, to follow a query that spreads them.
    fn fragments(self) -> String {
        match self {
            Self::Status => STATUS_FRAGMENT.to_string(),
            Self::List => format!("{LIST_FRAGMENT}{STATUS_FRAGMENT}"),
        }
    }
}

/// A search for the PRs `$q` finds, `$first` at a time. The user's query is only ever sent in a
/// variable, so what is typed can't change the shape of the query.
pub fn search_query(fields: Fields) -> String {
    format!(
        "query($q: String!, $first: Int!, $after: String) {{
  search(query: $q, type: ISSUE_ADVANCED, first: $first, after: $after) {{
    pageInfo {{ hasNextPage endCursor }}
    nodes {{ ...{} }}
  }}
}}
{}",
        fields.name(),
        fields.fragments()
    )
}

/// The key of a PR in the answer to [`lookup_query`].
fn alias(number: u64) -> String {
    format!("pr{number}")
}

/// A query for these PRs by number, all in one request. The numbers are written into the query,
/// which is safe as they are numbers. The repository is in the variables `$owner` and `$name`.
pub fn lookup_query(numbers: &[u64], fields: Fields) -> String {
    let lookups: String = numbers
        .iter()
        .map(|&number| {
            format!(
                "    {}: pullRequest(number: {number}) {{ ...{} }}\n",
                alias(number),
                fields.name()
            )
        })
        .collect();
    format!(
        "query($owner: String!, $name: String!) {{
  repository(owner: $owner, name: $name) {{
{lookups}  }}
}}
{}",
        fields.fragments()
    )
}

/// The search string for a user's `query`.
pub fn search_string(query: &str) -> String {
    format!("{SEARCH_PREFIX} {query}")
}

#[derive(Debug, Deserialize)]
pub struct Response<T> {
    pub data: T,
}

#[derive(Debug, Deserialize)]
pub struct SearchData<T> {
    pub search: Search<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Search<T> {
    pub page_info: PageInfo,
    pub nodes: Vec<Node<T>>,
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
pub enum Node<T> {
    Pr(Box<T>),
    Other(IgnoredAny),
}

/// The answer to [`lookup_query`]: each PR under its alias, or `null` for one that wasn't found.
#[derive(Debug, Deserialize)]
pub struct LookupData<T> {
    pub repository: HashMap<String, Option<T>>,
}

impl<T> LookupData<T> {
    /// The PR with this number, if the answer has it.
    pub fn take(&mut self, number: u64) -> Option<T> {
        self.repository.remove(&alias(number)).flatten()
    }
}

/// What a refresh brings for a PR.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlStatus {
    number: u64,
    state: PrState,
    mergeable: Mergeable,
    /// `null` when no review is required, where `gh` gave `""`.
    review_decision: Option<ReviewDecision>,
    updated_at: String,
    commits: Commits,
}

/// A PR as the table lists it.
#[derive(Debug, Deserialize)]
pub struct GqlPr {
    #[serde(flatten)]
    status: GqlStatus,
    title: String,
    url: String,
    /// `null` for an account that has been deleted.
    author: Option<GqlActor>,
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

impl From<GqlStatus> for PrStatus {
    fn from(status: GqlStatus) -> Self {
        let checks = status
            .commits
            .nodes
            .into_iter()
            .next()
            .and_then(|node| node.commit.status_check_rollup)
            .map(|rollup| rollup.contexts.nodes)
            .unwrap_or_default();
        PrStatus {
            number: status.number,
            state: status.state,
            status_check_rollup: checks.into_iter().map(Check::from).collect(),
            mergeable: status.mergeable,
            review_decision: status.review_decision.unwrap_or(ReviewDecision::Other),
            updated_at: status.updated_at,
        }
    }
}

impl From<GqlPr> for Pr {
    fn from(pr: GqlPr) -> Self {
        let status = PrStatus::from(pr.status);
        Pr {
            number: status.number,
            title: pr.title,
            state: status.state,
            status_check_rollup: status.status_check_rollup,
            mergeable: status.mergeable,
            review_decision: status.review_decision,
            url: pr.url,
            updated_at: status.updated_at,
            author: Author {
                login: login(pr.author),
            },
        }
    }
}

impl<T> Search<T> {
    /// The PRs of this page, leaving out anything that isn't one.
    pub fn prs(self) -> Vec<T> {
        self.nodes
            .into_iter()
            .filter_map(|node| match node {
                Node::Pr(pr) => Some(*pr),
                Node::Other(_) => None,
            })
            .collect()
    }
}

/// The details of one PR: its first `$after`-onwards page of comments, and the PR's own fields,
/// which every page repeats. The repository is in `$owner` and `$name`.
pub const DETAILS_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!, $after: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      headRefName
      body
      additions
      deletions
      comments(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes { author { __typename login } createdAt body url }
      }
    }
  }
}
"#;

#[derive(Debug, Deserialize)]
pub struct DetailsData {
    repository: DetailsRepository,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailsRepository {
    pull_request: GqlDetails,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlDetails {
    head_ref_name: String,
    body: Option<String>,
    additions: u64,
    deletions: u64,
    comments: GqlComments,
}

#[derive(Debug, Deserialize)]
struct GqlComments {
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
    nodes: Vec<GqlComment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlComment {
    /// `null` for an account that has been deleted, as `gh` gave it.
    author: Option<GqlActor>,
    created_at: String,
    body: Option<String>,
    url: Option<String>,
}

impl From<GqlComment> for Comment {
    fn from(comment: GqlComment) -> Self {
        Comment {
            author: comment.author.map(|actor| Author {
                login: login(Some(actor)),
            }),
            created_at: comment.created_at,
            body: comment.body,
            url: comment.url,
        }
    }
}

impl DetailsData {
    /// The details with this page of comments, and the cursor of the next page if there is one.
    pub fn into_page(self) -> (PrDetails, Option<String>) {
        let pr = self.repository.pull_request;
        let next = pr
            .comments
            .page_info
            .end_cursor
            .filter(|_| pr.comments.page_info.has_next_page);
        let details = PrDetails {
            head_ref_name: pr.head_ref_name,
            body: pr.body,
            comments: pr.comments.nodes.into_iter().map(Comment::from).collect(),
            additions: pr.additions,
            deletions: pr.deletions,
        };
        (details, next)
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

    fn page_of<T: serde::de::DeserializeOwned>() -> Search<T> {
        serde_json::from_str::<Response<SearchData<T>>>(PAGE)
            .unwrap()
            .data
            .search
    }

    fn page() -> Search<GqlPr> {
        page_of()
    }

    fn prs() -> Vec<Pr> {
        page().prs().into_iter().map(Pr::from).collect()
    }

    #[test]
    fn a_page_gives_its_cursor_and_leaves_out_what_is_not_a_pr() {
        let search = page();
        assert!(search.page_info.has_next_page);
        assert_eq!(search.page_info.end_cursor.as_deref(), Some("abc"));
        let numbers: Vec<u64> = search
            .prs()
            .into_iter()
            .map(|pr| Pr::from(pr).number)
            .collect();
        assert_eq!(numbers, [12, 13, 14]);
    }

    #[test]
    fn a_bot_login_has_the_app_prefix_and_a_deleted_account_is_ghost() {
        let prs = prs();
        assert_eq!(prs[0].author.login, "app/renovate");
        assert_eq!(prs[1].author.login, "alice");
        assert_eq!(prs[2].author.login, "ghost");
    }

    #[test]
    fn checks_get_the_workflow_name_and_the_status_start_that_gh_gave() {
        let prs = prs();
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
        let prs = prs();
        assert!(prs[1].status_check_rollup.is_empty());
        assert!(prs[2].status_check_rollup.is_empty());
    }

    #[test]
    fn a_missing_review_decision_is_what_an_empty_string_was() {
        let prs = prs();
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

    #[test]
    fn a_status_search_gives_the_fields_a_refresh_brings() {
        let search: Search<GqlStatus> = page_of();
        let statuses: Vec<PrStatus> = search.prs().into_iter().map(PrStatus::from).collect();
        assert_eq!(statuses.len(), 3);
        assert_eq!(statuses[0].number, 12);
        assert_eq!(statuses[0].status_check_rollup.len(), 4);
        assert_eq!(statuses[0].review_decision, ReviewDecision::Other);
        assert_eq!(statuses[1].state, PrState::Merged);
        assert_eq!(statuses[2].mergeable, Mergeable::Conflicting);
        assert_eq!(statuses[2].updated_at, "2026-01-02T03:04:08Z");
    }

    #[test]
    fn a_status_and_a_listed_pr_have_the_same_checks() {
        let status: Vec<PrStatus> = page_of::<GqlStatus>()
            .prs()
            .into_iter()
            .map(PrStatus::from)
            .collect();
        let listed = prs();
        for (status, pr) in status.iter().zip(&listed) {
            assert_eq!(status.number, pr.number);
            assert_eq!(
                format!("{:?}", status.status_check_rollup),
                format!("{:?}", pr.status_check_rollup)
            );
        }
    }

    #[test]
    fn every_query_defines_the_fragments_it_uses_and_no_others() {
        // GitHub refuses a query with a fragment it doesn't spread, or spreads and doesn't define.
        for fields in [Fields::Status, Fields::List] {
            for query in [search_query(fields), lookup_query(&[1, 2], fields)] {
                assert!(query.starts_with("query"));
                assert!(query.contains(&format!("...{}", fields.name())));
                assert!(query.contains("fragment StatusFields on PullRequest"));
                let list = query.contains("fragment ListFields on PullRequest");
                assert_eq!(list, matches!(fields, Fields::List), "{query}");
            }
        }
    }

    #[test]
    fn a_lookup_asks_for_each_number_under_its_own_alias() {
        let query = lookup_query(&[12, 7], Fields::Status);
        assert!(query.contains("pr12: pullRequest(number: 12) { ...StatusFields }"));
        assert!(query.contains("pr7: pullRequest(number: 7) { ...StatusFields }"));
        assert!(query.contains("repository(owner: $owner, name: $name)"));
        // The user's search text is never part of a query, only of its variables.
        assert!(!search_query(Fields::List).contains("repo:"));
    }

    #[test]
    fn a_lookup_answer_gives_each_pr_it_has_and_skips_a_null() {
        let json = r#"{"data": {"repository": {
          "pr12": {"number": 12, "state": "OPEN", "mergeable": "MERGEABLE",
                   "reviewDecision": null, "updatedAt": "2026-01-02T03:04:06Z",
                   "commits": {"nodes": []}},
          "pr7": null
        }}}"#;
        let mut data = serde_json::from_str::<Response<LookupData<GqlStatus>>>(json)
            .unwrap()
            .data;
        assert!(data.take(7).is_none());
        assert!(data.take(99).is_none());
        let status = PrStatus::from(data.take(12).unwrap());
        assert_eq!(status.number, 12);
        assert!(status.status_check_rollup.is_empty());
    }

    fn details_page(has_next: bool, cursor: &str) -> DetailsData {
        let json = format!(
            r#"{{"repository": {{"pullRequest": {{
              "headRefName": "renovate/example-lib-2.x",
              "body": "",
              "additions": 3,
              "deletions": 1,
              "comments": {{
                "pageInfo": {{"hasNextPage": {has_next}, "endCursor": "{cursor}"}},
                "nodes": [
                  {{"author": {{"__typename": "Bot", "login": "renovate"}},
                    "createdAt": "2026-01-02T03:04:05Z", "body": "hello",
                    "url": "https://example.test/pull/1#issuecomment-1"}},
                  {{"author": null, "createdAt": "2026-01-02T03:04:06Z", "body": "gone",
                    "url": null}}
                ]
              }}
            }}}}}}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn details_have_the_comments_authors_as_gh_gave_them() {
        let (details, _) = details_page(false, "x").into_page();
        assert_eq!(details.head_ref_name, "renovate/example-lib-2.x");
        assert_eq!((details.additions, details.deletions), (3, 1));
        assert_eq!(details.comments.len(), 2);
        assert_eq!(
            details.comments[0].author.as_ref().unwrap().login,
            "app/renovate"
        );
        assert_eq!(
            details.comments[0].url.as_deref(),
            Some("https://example.test/pull/1#issuecomment-1")
        );
        // A deleted account has no author and the comment keeps its text.
        assert!(details.comments[1].author.is_none());
        assert_eq!(details.comments[1].body.as_deref(), Some("gone"));
        assert!(details.comments[1].url.is_none());
    }

    #[test]
    fn a_page_of_comments_gives_the_cursor_of_the_next_only_when_there_is_one() {
        assert_eq!(
            details_page(true, "next").into_page().1.as_deref(),
            Some("next")
        );
        assert_eq!(details_page(false, "last").into_page().1, None);
    }
}
