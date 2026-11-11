//! The reads, as GraphQL queries sent with `gh api graphql`: the queries, the raw shape GitHub
//! answers in, and what turns that into the types the rest of the program uses. `gh pr list` did
//! several of those conversions for us, so they are done here and tested.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde::de::IgnoredAny;

use super::{
    Approval, Approvers, Author, Check, CheckRun, Comment, CommentKind, Conclusion, ContextState,
    Mergeable, Pr, PrDetails, PrState, PrStatus, Reply, ReviewDecision, ReviewRequest, Reviewer,
    RunStatus, StatusContext, Thread, Verdict,
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
  baseRefName
  commits(last: 1) {
    nodes {
      commit {
        # GitHub judges a workflow by its newest suite, so a suite that was cancelled before it
        # reported anything hides the earlier runs. 15368 is the GitHub Actions app.
        checkSuites(last: 100, filterBy: {appId: 15368}) {
          nodes {
            createdAt
            conclusion
            workflowRun { workflow { name } }
            checkRuns(first: 1) { totalCount }
          }
        }
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
/// which is safe as they are numbers. A number given twice is asked for once, as GraphQL rejects a
/// query that uses an alias twice. The repository is in the variables `$owner` and `$name`.
pub fn lookup_query(numbers: &[u64], fields: Fields) -> String {
    let mut seen = std::collections::HashSet::new();
    let lookups: String = numbers
        .iter()
        .filter(|&&number| seen.insert(number))
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
    base_ref_name: Option<String>,
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
    #[serde(default)]
    check_suites: Option<Suites>,
}

#[derive(Debug, Deserialize)]
struct Suites {
    nodes: Vec<Suite>,
}

/// A group of check runs, one per workflow run.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Suite {
    created_at: String,
    /// `null` until the suite completes.
    conclusion: Option<Conclusion>,
    /// `null` for a suite that no workflow made.
    workflow_run: Option<WorkflowRun>,
    check_runs: SuiteRuns,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuiteRuns {
    total_count: u64,
}

/// The workflows whose newest suite has reported no checks and didn't end well, usually because
/// it was cancelled before its jobs were created. GitHub shows the checks of such a workflow as
/// "Expected — Waiting for status to be reported", whatever its earlier suites gave.
fn unreported_workflows(suites: Option<Suites>) -> HashSet<String> {
    let mut newest: HashMap<String, Suite> = HashMap::new();
    for suite in suites.into_iter().flat_map(|suites| suites.nodes) {
        let Some(workflow) = suite
            .workflow_run
            .as_ref()
            .map(|run| run.workflow.name.clone())
        else {
            continue;
        };
        // On a tie the later suite wins: GitHub lists them oldest first.
        if newest
            .get(&workflow)
            .is_none_or(|seen| suite.created_at >= seen.created_at)
        {
            newest.insert(workflow, suite);
        }
    }
    newest
        .into_iter()
        .filter(|(_, suite)| {
            suite.check_runs.total_count == 0
                && !matches!(
                    suite.conclusion,
                    Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped)
                )
        })
        .map(|(workflow, _)| workflow)
        .collect()
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

/// What a query for PRs gives, so that the checks the base branch requires can be added to it.
pub trait Required {
    /// The branch the PR merges into.
    fn base_branch(&self) -> Option<&str>;
    /// Adds a pending check for each of `contexts` that the PR has no check for. A required check
    /// that hasn't started isn't in GitHub's rollup, but it blocks the merge, so it is shown as
    /// pending, as on GitHub's page.
    fn add_required(&mut self, contexts: &[String]);
}

impl Required for GqlStatus {
    fn base_branch(&self) -> Option<&str> {
        self.base_ref_name.as_deref()
    }

    fn add_required(&mut self, contexts: &[String]) {
        let Some(node) = self.commits.nodes.first_mut() else {
            return;
        };
        let rollup = node.commit.status_check_rollup.get_or_insert(Rollup {
            contexts: Contexts { nodes: Vec::new() },
        });
        for context in contexts {
            let seen = rollup.contexts.nodes.iter().any(|check| match check {
                GqlCheck::CheckRun(run) => run.name == *context,
                GqlCheck::StatusContext(status) => status.context == *context,
                GqlCheck::Unknown => false,
            });
            if !seen {
                rollup
                    .contexts
                    .nodes
                    .push(GqlCheck::StatusContext(GqlStatusContext {
                        context: context.clone(),
                        state: ContextState::Expected,
                        created_at: None,
                        target_url: None,
                    }));
            }
        }
    }
}

impl Required for GqlPr {
    fn base_branch(&self) -> Option<&str> {
        self.status.base_branch()
    }

    fn add_required(&mut self, contexts: &[String]) {
        self.status.add_required(contexts);
    }
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
        let commit = status
            .commits
            .nodes
            .into_iter()
            .next()
            .map(|node| node.commit);
        let (rollup, suites) = match commit {
            Some(commit) => (commit.status_check_rollup, commit.check_suites),
            None => (None, None),
        };
        let unreported = unreported_workflows(suites);
        let checks = rollup
            .map(|rollup| rollup.contexts.nodes)
            .unwrap_or_default();
        PrStatus {
            number: status.number,
            state: status.state,
            status_check_rollup: checks
                .into_iter()
                .map(Check::from)
                .map(|mut check| {
                    if let Check::CheckRun(run) = &mut check
                        && unreported.contains(&run.workflow_name)
                    {
                        run.status = RunStatus::Other;
                    }
                    check
                })
                .collect(),
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

/// The details of one PR: a page of its comments, from `$after` on, and the PR's own fields, which
/// every page repeats. The reviews and review threads, up to 100 of each and 100 comments in a
/// thread, and who reviewed and who is asked (up to 50 of each), come with the first page only
/// (`$withReviews`), as they have no paging here. The
/// inline comments are taken from the threads alone: a review's own `comments` are the same ones.
/// The repository is in `$owner` and `$name`.
pub const DETAILS_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!, $after: String, $withReviews: Boolean!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      headRefName
      headRefOid
      body
      additions
      deletions
      reviewRequests(first: 50) @include(if: $withReviews) {
        nodes {
          asCodeOwner
          requestedReviewer {
            __typename
            ... on User { login }
            ... on Bot { login }
            ... on Mannequin { login }
            ... on Team { combinedSlug }
          }
        }
      }
      latestOpinionatedReviews(first: 50) @include(if: $withReviews) {
        nodes {
          state
          author { __typename login }
          commit { oid }
          onBehalfOf(first: 10) { nodes { combinedSlug } }
        }
      }
      comments(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes { author { __typename login } createdAt body url }
      }
      reviews(first: 100) @include(if: $withReviews) {
        nodes { author { __typename login } state body submittedAt url }
      }
      reviewThreads(first: 100) @include(if: $withReviews) {
        nodes {
          isResolved
          isOutdated
          path
          line
          originalLine
          comments(first: 100) {
            nodes { author { __typename login } createdAt body url }
          }
        }
      }
    }
  }
}
"#;

/// Whether the viewer's own latest review is an approval of the PR's current head. The PR is
/// `$number` in `$owner` and `$name`.
pub const VIEWER_APPROVAL_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!) {
  viewer { login }
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      headRefOid
      latestOpinionatedReviews(first: 50) {
        nodes { state author { login } commit { oid } }
      }
    }
  }
}
"#;

#[derive(Debug, Deserialize)]
pub struct ViewerApprovalData {
    viewer: ViewerLogin,
    repository: ViewerApprovalRepository,
}

#[derive(Debug, Deserialize)]
struct ViewerLogin {
    login: String,
}

#[derive(Debug, Deserialize)]
struct ViewerApprovalRepository {
    #[serde(rename = "pullRequest")]
    pull_request: ViewerApprovalPr,
}

#[derive(Debug, Deserialize)]
struct ViewerApprovalPr {
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
    #[serde(rename = "latestOpinionatedReviews")]
    reviews: Nodes<ViewerReview>,
}

#[derive(Debug, Deserialize)]
struct ViewerReview {
    state: ReviewState,
    author: Option<ViewerLogin>,
    commit: Option<GqlCommitId>,
}

impl ViewerApprovalData {
    /// Whether the viewer has approved the PR's current head, so approving again adds nothing.
    pub fn already_approved(&self) -> bool {
        let pr = &self.repository.pull_request;
        pr.reviews.nodes.iter().any(|review| {
            matches!(review.state, ReviewState::Approved)
                && review
                    .author
                    .as_ref()
                    .is_some_and(|a| a.login == self.viewer.login)
                && review
                    .commit
                    .as_ref()
                    .is_some_and(|c| c.oid == pr.head_ref_oid)
        })
    }
}

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
    head_ref_oid: String,
    body: Option<String>,
    additions: u64,
    deletions: u64,
    comments: GqlComments,
    /// Left out of the answer, not `null`, after the first page.
    #[serde(default)]
    reviews: Option<Nodes<GqlReview>>,
    #[serde(default)]
    review_threads: Option<Nodes<GqlThread>>,
    #[serde(default)]
    review_requests: Option<Nodes<GqlReviewRequest>>,
    #[serde(default)]
    latest_opinionated_reviews: Option<Nodes<GqlApproval>>,
}

/// Who a review is asked of. `null` for a reviewer the account can't see.
#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
enum GqlReviewer {
    User {
        login: String,
    },
    Bot {
        login: String,
    },
    Mannequin {
        login: String,
    },
    Team {
        #[serde(rename = "combinedSlug")]
        combined_slug: String,
    },
    /// Any other kind, such as an enterprise team.
    #[serde(other)]
    Other,
}

impl GqlReviewer {
    fn into_reviewer(self) -> Reviewer {
        match self {
            Self::User { login } | Self::Mannequin { login } => Reviewer::Person(login),
            Self::Bot { login } => Reviewer::Person(format!("app/{login}")),
            Self::Team { combined_slug } => Reviewer::Team(combined_slug),
            Self::Other => Reviewer::Hidden,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlReviewRequest {
    as_code_owner: bool,
    requested_reviewer: Option<GqlReviewer>,
}

#[derive(Debug, Deserialize)]
struct GqlApproval {
    state: ReviewState,
    author: Option<GqlActor>,
    /// `null` for a commit that is gone.
    commit: Option<GqlCommitId>,
    #[serde(rename = "onBehalfOf")]
    on_behalf_of: Nodes<GqlTeamId>,
}

#[derive(Debug, Deserialize)]
struct GqlCommitId {
    oid: String,
}

#[derive(Debug, Deserialize)]
struct GqlTeamId {
    #[serde(rename = "combinedSlug")]
    combined_slug: String,
}

impl GqlApproval {
    /// The review as an approval, or `None` for one that says nothing about the PR.
    fn into_approval(self, head: &str) -> Option<Approval> {
        let verdict = match self.state {
            ReviewState::Approved => Verdict::Approved,
            ReviewState::ChangesRequested => Verdict::ChangesRequested,
            ReviewState::Dismissed => Verdict::Dismissed,
            ReviewState::Commented | ReviewState::Pending => return None,
        };
        Some(Approval {
            by: self.author.map_or(Reviewer::Hidden, |actor| {
                Reviewer::Person(login(Some(actor)))
            }),
            verdict,
            stale: self.commit.is_some_and(|commit| commit.oid != head),
            for_teams: self
                .on_behalf_of
                .nodes
                .into_iter()
                .map(|team| team.combined_slug)
                .collect(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

/// A review's state as GitHub gives it. `PENDING` is the viewer's own review, not yet submitted;
/// it, or any state added later, is left out.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
    #[serde(other)]
    Pending,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlReview {
    author: Option<GqlActor>,
    state: ReviewState,
    body: String,
    submitted_at: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlThread {
    is_resolved: bool,
    is_outdated: bool,
    path: String,
    /// `null` once the code has changed.
    line: Option<u64>,
    original_line: Option<u64>,
    comments: Nodes<GqlComment>,
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

/// Puts the comments in the order they were written (a review by when it was submitted, a thread
/// by its first comment), so new activity comes last and the comments already listed keep their
/// places. The sort is stable, so ones from the same moment stay in the order fetched.
pub fn sort_comments(comments: &mut [Comment]) {
    comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
}

/// A comment's author: `None` for a deleted account, as `gh` gave it.
fn author(actor: Option<GqlActor>) -> Option<Author> {
    actor.map(|actor| Author {
        login: login(Some(actor)),
    })
}

impl From<GqlComment> for Comment {
    fn from(comment: GqlComment) -> Self {
        Comment {
            author: author(comment.author),
            created_at: comment.created_at,
            body: comment.body,
            url: comment.url,
            kind: CommentKind::Comment,
            replies: Vec::new(),
        }
    }
}

impl GqlReview {
    /// The review as a comment, or `None` if it has nothing to read: one with no text is a
    /// wrapper for the thread comments made with it, or a bare approval, which is for the
    /// approvers. A review not yet submitted is the viewer's own draft.
    fn into_comment(self) -> Option<Comment> {
        let verdict = match self.state {
            ReviewState::Approved => Verdict::Approved,
            ReviewState::ChangesRequested => Verdict::ChangesRequested,
            ReviewState::Commented => Verdict::Commented,
            ReviewState::Dismissed => Verdict::Dismissed,
            ReviewState::Pending => return None,
        };
        let created_at = self.submitted_at?;
        if self.body.trim().is_empty() {
            return None;
        }
        Some(Comment {
            author: author(self.author),
            created_at,
            body: Some(self.body),
            url: self.url,
            kind: CommentKind::Review(verdict),
            replies: Vec::new(),
        })
    }
}

impl GqlThread {
    /// The thread as one comment, its first comment's, with the rest as replies, or `None` for a
    /// thread with no comments.
    fn into_comment(self) -> Option<Comment> {
        let mut comments = self.comments.nodes.into_iter();
        let first = comments.next()?;
        let replies = comments
            .map(|reply| Reply {
                author: author(reply.author),
                created_at: reply.created_at,
                body: reply.body,
            })
            .collect();
        Some(Comment {
            author: author(first.author),
            created_at: first.created_at,
            body: first.body,
            url: first.url,
            kind: CommentKind::Thread(Thread {
                path: self.path,
                line: self.line.or(self.original_line),
                resolved: self.is_resolved,
                outdated: self.is_outdated,
            }),
            replies,
        })
    }
}

impl DetailsData {
    /// The details with this page of comments, and the cursor of the next page if there is one.
    /// The first page also has the reviews and threads, which come after the comments on the PR;
    /// the caller sorts them all by time once it has every page.
    pub fn into_page(self) -> (PrDetails, Option<String>) {
        let pr = self.repository.pull_request;
        let next = pr
            .comments
            .page_info
            .end_cursor
            .filter(|_| pr.comments.page_info.has_next_page);
        let mut comments: Vec<Comment> = pr.comments.nodes.into_iter().map(Comment::from).collect();
        let reviews = pr.reviews.into_iter().flat_map(|reviews| reviews.nodes);
        comments.extend(reviews.filter_map(GqlReview::into_comment));
        let threads = pr
            .review_threads
            .into_iter()
            .flat_map(|threads| threads.nodes);
        comments.extend(threads.filter_map(GqlThread::into_comment));
        let head = pr.head_ref_oid;
        let approvers = Approvers {
            approvals: pr
                .latest_opinionated_reviews
                .into_iter()
                .flat_map(|reviews| reviews.nodes)
                .filter_map(|review| review.into_approval(&head))
                .collect(),
            requests: pr
                .review_requests
                .into_iter()
                .flat_map(|requests| requests.nodes)
                .map(|request| ReviewRequest {
                    of: request
                        .requested_reviewer
                        .map_or(Reviewer::Hidden, GqlReviewer::into_reviewer),
                    code_owner: request.as_code_owner,
                })
                .collect(),
        };
        let details = PrDetails {
            head_ref_name: pr.head_ref_name,
            body: pr.body,
            comments,
            additions: pr.additions,
            deletions: pr.deletions,
            approvers,
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

    fn viewer_approval(state: &str, author: &str, oid: &str) -> bool {
        let json = format!(
            r#"{{"viewer": {{"login": "me"}}, "repository": {{"pullRequest": {{"headRefOid": "new",
            "latestOpinionatedReviews": {{"nodes": [
              {{"state": "{state}", "author": {{"login": "{author}"}}, "commit": {{"oid": "{oid}"}}}}
            ]}}}}}}}}"#
        );
        serde_json::from_str::<ViewerApprovalData>(&json)
            .unwrap()
            .already_approved()
    }

    #[test]
    fn only_the_viewers_approval_of_the_current_head_counts() {
        assert!(viewer_approval("APPROVED", "me", "new"));
        assert!(!viewer_approval("APPROVED", "someone", "new"));
        assert!(!viewer_approval("APPROVED", "me", "old"));
        assert!(!viewer_approval("CHANGES_REQUESTED", "me", "new"));
    }

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

    /// The PR of this number in `PAGE`.
    fn pr(number: u64) -> Pr {
        prs()
            .into_iter()
            .find(|pr| pr.number == number)
            .unwrap_or_else(|| panic!("PAGE has no PR {number}"))
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
        assert_eq!(pr(12).author.login, "app/renovate");
        assert_eq!(pr(13).author.login, "alice");
        assert_eq!(pr(14).author.login, "ghost");
    }

    #[test]
    fn checks_get_the_workflow_name_and_the_status_start_that_gh_gave() {
        let checks = pr(12).status_check_rollup;
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

    fn suite(created_at: &str, conclusion: Option<Conclusion>, runs: u64) -> Suite {
        Suite {
            created_at: created_at.to_string(),
            conclusion,
            workflow_run: Some(WorkflowRun {
                workflow: Workflow {
                    name: "Flight".to_string(),
                },
            }),
            check_runs: SuiteRuns { total_count: runs },
        }
    }

    fn unreported(suites: Vec<Suite>) -> bool {
        unreported_workflows(Some(Suites { nodes: suites })).contains("Flight")
    }

    #[test]
    fn a_workflow_whose_newest_suite_reported_nothing_is_unreported() {
        // As on containers #17617: a success, then a suite cancelled before it had any jobs.
        let cancelled = Some(Conclusion::Other);
        let success = Some(Conclusion::Success);
        assert!(unreported(vec![
            suite("2026-11-04T05:18:09Z", success, 1),
            suite("2026-11-04T05:18:09Z", cancelled, 0),
        ]));
        // Not started at all.
        assert!(unreported(vec![suite("2026-11-04T05:18:09Z", None, 0)]));
        // A newer suite that did report, or an older empty one, doesn't count.
        assert!(!unreported(vec![
            suite("2026-11-04T05:18:08Z", cancelled, 0),
            suite("2026-11-04T05:18:09Z", success, 1),
        ]));
        // Nothing to report, but nothing wrong.
        assert!(!unreported(vec![suite("2026-11-04T05:18:09Z", success, 0)]));
        assert!(unreported_workflows(None).is_empty());
    }

    #[test]
    fn the_checks_of_an_unreported_workflow_are_pending() {
        let json = status_json(
            r#"{"contexts": {"nodes": [
              {"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
               "conclusion": "SUCCESS", "startedAt": null, "detailsUrl": null,
               "checkSuite": {"workflowRun": {"workflow": {"name": "CI"}}}}
            ]}}"#,
            r#"{"nodes": [
              {"createdAt": "2026-01-02T03:00:00Z", "conclusion": "SUCCESS",
               "workflowRun": {"workflow": {"name": "CI"}}, "checkRuns": {"totalCount": 1}},
              {"createdAt": "2026-01-02T03:00:01Z", "conclusion": "CANCELLED",
               "workflowRun": {"workflow": {"name": "CI"}}, "checkRuns": {"totalCount": 0}}
            ]}"#,
        );
        let status = PrStatus::from(serde_json::from_str::<GqlStatus>(&json).unwrap());
        assert!(matches!(
            &status.status_check_rollup[..],
            [Check::CheckRun(run)] if run.name == "build" && run.status == RunStatus::Other
        ));
    }

    #[test]
    fn no_checks_or_no_commit_means_an_empty_rollup() {
        assert!(pr(13).status_check_rollup.is_empty());
        assert!(pr(14).status_check_rollup.is_empty());
    }

    #[test]
    fn a_missing_review_decision_is_what_an_empty_string_was() {
        assert_eq!(pr(12).review_decision, ReviewDecision::Other);
        assert_eq!(pr(13).review_decision, ReviewDecision::Other);
    }

    #[test]
    fn a_listed_pr_keeps_its_review_decision_mergeable_and_state() {
        assert_eq!(pr(14).review_decision, ReviewDecision::Approved);
        assert_eq!(pr(14).mergeable, Mergeable::Conflicting);
        assert_eq!(pr(13).state, PrState::Merged);
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
    fn a_status_and_a_listed_pr_have_the_same_checks() {
        let statuses: Vec<PrStatus> = page_of::<GqlStatus>()
            .prs()
            .into_iter()
            .map(PrStatus::from)
            .collect();
        assert_eq!(statuses.len(), 3);
        for status in &statuses {
            assert_eq!(
                status.status_check_rollup,
                pr(status.number).status_check_rollup,
                "PR {}",
                status.number
            );
        }
        // And the other fields a refresh brings.
        assert_eq!(statuses[0].number, 12);
        assert_eq!(statuses[0].status_check_rollup.len(), 4);
        assert_eq!(statuses[0].review_decision, ReviewDecision::Other);
        assert_eq!(statuses[1].state, PrState::Merged);
        assert_eq!(statuses[2].mergeable, Mergeable::Conflicting);
        assert_eq!(statuses[2].updated_at, "2026-01-02T03:04:08Z");
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
    }

    #[test]
    fn the_users_text_is_never_in_the_query() {
        // It is only in the variables.
        assert!(!search_query(Fields::List).contains("repo:"));
    }

    #[test]
    fn a_lookup_asks_for_a_repeated_number_only_once() {
        // GraphQL rejects a query that uses an alias twice.
        let query = lookup_query(&[5, 7, 5], Fields::Status);
        assert_eq!(query.matches("pr5: ").count(), 1);
        assert_eq!(query.matches("pr7: ").count(), 1);
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
              "headRefName": "renovate/example-lib-2.x", "headRefOid": "head1",
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

    /// A first page with a comment, then every kind of review and thread there is.
    const REVIEWED: &str = r#"{"repository": {"pullRequest": {
      "headRefName": "b", "headRefOid": "head1", "body": "", "additions": 1, "deletions": 1,
      "comments": {
        "pageInfo": {"hasNextPage": false, "endCursor": null},
        "nodes": [{"author": {"__typename": "User", "login": "alice"},
                   "createdAt": "2026-01-02T05:00:00Z", "body": "on the PR", "url": null}]
      },
      "reviews": {"nodes": [
        {"author": {"__typename": "User", "login": "bob"}, "state": "APPROVED",
         "body": "Looks good", "submittedAt": "2026-01-02T06:00:00Z", "url": "https://r/1"},
        {"author": {"__typename": "User", "login": "bob"}, "state": "COMMENTED",
         "body": "", "submittedAt": "2026-01-02T01:00:00Z", "url": "https://r/2"},
        {"author": {"__typename": "User", "login": "bob"}, "state": "APPROVED",
         "body": "  \n", "submittedAt": "2026-01-02T01:30:00Z", "url": "https://r/3"},
        {"author": {"__typename": "User", "login": "me"}, "state": "PENDING",
         "body": "draft", "submittedAt": null, "url": null}
      ]},
      "reviewThreads": {"nodes": [
        {"isResolved": true, "isOutdated": false, "path": "src/a.rs", "line": 12,
         "originalLine": 10, "comments": {"nodes": [
           {"author": {"__typename": "Bot", "login": "reviewer"},
            "createdAt": "2026-01-02T04:00:00Z", "body": "why?", "url": "https://t/1"},
           {"author": null, "createdAt": "2026-01-02T04:30:00Z", "body": "because",
            "url": "https://t/2"}]}},
        {"isResolved": false, "isOutdated": true, "path": "src/b.rs", "line": null,
         "originalLine": 7, "comments": {"nodes": [
           {"author": {"__typename": "User", "login": "carol"},
            "createdAt": "2026-01-02T07:00:00Z", "body": "old", "url": "https://t/3"}]}},
        {"isResolved": false, "isOutdated": false, "path": "src/c.rs", "line": null,
         "originalLine": null, "comments": {"nodes": []}}
      ]}
    }}}"#;

    /// The comment of `details` with this text, a review or a thread among them.
    fn comment_with<'a>(details: &'a PrDetails, body: &str) -> &'a Comment {
        details
            .comments
            .iter()
            .find(|c| c.body.as_deref() == Some(body))
            .unwrap_or_else(|| panic!("no comment says {body:?}"))
    }

    #[test]
    fn reviews_and_threads_join_the_comments_in_the_order_they_were_written() {
        let data: DetailsData = serde_json::from_str(REVIEWED).unwrap();
        let (mut details, _) = data.into_page();
        sort_comments(&mut details.comments);
        let summary: Vec<(&str, &str)> = details
            .comments
            .iter()
            .map(|c| (c.created_at.as_str(), c.body.as_deref().unwrap()))
            .collect();
        assert_eq!(
            summary,
            [
                ("2026-01-02T04:00:00Z", "why?"),
                ("2026-01-02T05:00:00Z", "on the PR"),
                ("2026-01-02T06:00:00Z", "Looks good"),
                ("2026-01-02T07:00:00Z", "old"),
            ]
        );
    }

    #[test]
    fn a_review_with_no_text_one_not_yet_submitted_and_an_empty_thread_are_left_out() {
        let data: DetailsData = serde_json::from_str(REVIEWED).unwrap();
        let (details, _) = data.into_page();
        // Four of the eight things written: the comment, the review with text and the two threads that have comments.
        assert_eq!(details.comments.len(), 4);
        for body in ["", "  \n", "draft"] {
            assert!(
                details
                    .comments
                    .iter()
                    .all(|c| c.body.as_deref() != Some(body)),
                "{body:?} is listed"
            );
        }
    }

    #[test]
    fn a_thread_is_its_first_comment_with_the_rest_as_replies() {
        let data: DetailsData = serde_json::from_str(REVIEWED).unwrap();
        let (details, _) = data.into_page();
        let thread = comment_with(&details, "why?");
        assert_eq!(thread.author.as_ref().unwrap().login, "app/reviewer");
        assert_eq!(thread.url.as_deref(), Some("https://t/1"));
        assert_eq!(thread.replies.len(), 1);
        assert!(thread.replies[0].author.is_none());
        assert_eq!(thread.replies[0].body.as_deref(), Some("because"));
        assert_eq!(
            thread.tags(),
            ["src/a.rs:12", "1 reply", "resolved"].map(String::from)
        );
    }

    #[test]
    fn an_outdated_thread_keeps_the_line_it_was_on() {
        let data: DetailsData = serde_json::from_str(REVIEWED).unwrap();
        let (details, _) = data.into_page();
        let thread = comment_with(&details, "old");
        assert_eq!(thread.tags(), ["src/b.rs:7", "outdated"].map(String::from));
    }

    #[test]
    fn a_review_is_headed_by_how_it_ended() {
        let data: DetailsData = serde_json::from_str(REVIEWED).unwrap();
        let (details, _) = data.into_page();
        let review = comment_with(&details, "Looks good");
        assert_eq!(review.tags(), ["approved".to_string()]);
        assert_eq!(review.url.as_deref(), Some("https://r/1"));
    }

    /// A page after the first: no reviews, threads or approvers, because the query leaves their keys
    /// out (`@include(if: false)`).
    const LATER_PAGE: &str = r#"{"repository": {"pullRequest": {
      "headRefName": "b", "headRefOid": "head1", "body": null, "additions": 1, "deletions": 1,
      "comments": {"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": []}
    }}}"#;

    #[test]
    fn a_later_page_has_no_reviews_or_threads_and_still_parses() {
        let data: DetailsData = serde_json::from_str(LATER_PAGE).unwrap();
        let (details, next) = data.into_page();
        assert!(details.comments.is_empty() && next.is_none());
    }

    /// A PR at commit `head1` with every kind of reviewer and of review there is.
    const APPROVERS: &str = r#"{"repository": {"pullRequest": {
      "headRefName": "b", "headRefOid": "head1", "body": "", "additions": 1, "deletions": 1,
      "comments": {"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": []},
      "reviewRequests": {"nodes": [
        {"asCodeOwner": true, "requestedReviewer": {"__typename": "Team", "combinedSlug": "org/owners"}},
        {"asCodeOwner": false, "requestedReviewer": {"__typename": "User", "login": "dave"}},
        {"asCodeOwner": false, "requestedReviewer": {"__typename": "Bot", "login": "copilot"}},
        {"asCodeOwner": true, "requestedReviewer": null},
        {"asCodeOwner": false, "requestedReviewer": {"__typename": "EnterpriseTeam"}}
      ]},
      "latestOpinionatedReviews": {"nodes": [
        {"state": "APPROVED", "author": {"__typename": "User", "login": "alice"},
         "commit": {"oid": "head1"}, "onBehalfOf": {"nodes": [{"combinedSlug": "org/owners"}]}},
        {"state": "APPROVED", "author": {"__typename": "User", "login": "bob"},
         "commit": {"oid": "older"}, "onBehalfOf": {"nodes": []}},
        {"state": "CHANGES_REQUESTED", "author": {"__typename": "User", "login": "carol"},
         "commit": {"oid": "head1"}, "onBehalfOf": {"nodes": []}},
        {"state": "DISMISSED", "author": null, "commit": null, "onBehalfOf": {"nodes": []}},
        {"state": "COMMENTED", "author": {"__typename": "User", "login": "erin"},
         "commit": {"oid": "head1"}, "onBehalfOf": {"nodes": []}},
        {"state": "PENDING", "author": {"__typename": "User", "login": "me"},
         "commit": {"oid": "head1"}, "onBehalfOf": {"nodes": []}}
      ]}
    }}}"#;

    fn approvers() -> Approvers {
        let data: DetailsData = serde_json::from_str(APPROVERS).unwrap();
        data.into_page().0.approvers
    }

    #[test]
    fn approvals_say_who_how_and_whether_the_commit_is_old() {
        let approvals = approvers().approvals;
        // A comment-only review and a pending one say nothing about the PR.
        assert_eq!(approvals.len(), 4);
        let person = |name: &str| Reviewer::Person(name.to_string());
        assert_eq!(
            approvals[0],
            Approval {
                by: person("alice"),
                verdict: Verdict::Approved,
                stale: false,
                for_teams: vec!["org/owners".to_string()],
            }
        );
        // The commit is not the head, so the approval is of older code.
        assert!(approvals[1].stale && approvals[1].for_teams.is_empty());
        assert_eq!(approvals[2].verdict, Verdict::ChangesRequested);
        assert!(!approvals[2].stale);
        // A deleted account and a commit that is gone: not a person, and not called stale.
        assert_eq!(approvals[3].by, Reviewer::Hidden);
        assert_eq!(approvals[3].verdict, Verdict::Dismissed);
        assert!(!approvals[3].stale);
    }

    #[test]
    fn requests_tell_a_team_from_a_person_and_keep_the_code_owner_mark() {
        let requests = approvers().requests;
        let summary: Vec<(Reviewer, bool)> =
            requests.into_iter().map(|r| (r.of, r.code_owner)).collect();
        assert_eq!(
            summary,
            [
                (Reviewer::Team("org/owners".to_string()), true),
                (Reviewer::Person("dave".to_string()), false),
                (Reviewer::Person("app/copilot".to_string()), false),
                // One the account can't see, and a kind that isn't known.
                (Reviewer::Hidden, true),
                (Reviewer::Hidden, false),
            ]
        );
    }

    #[test]
    fn a_later_page_has_no_approvers() {
        let data: DetailsData = serde_json::from_str(LATER_PAGE).unwrap();
        assert_eq!(data.into_page().0.approvers, Approvers::default());
    }

    /// The status of PR 1 on `master`, whose latest commit has this `statusCheckRollup` and these
    /// `checkSuites` (JSON, or `null` for none).
    fn status_json(rollup: &str, suites: &str) -> String {
        format!(
            r#"{{
              "number": 1, "state": "OPEN", "mergeable": "MERGEABLE", "reviewDecision": null,
              "updatedAt": "2026-01-02T03:04:05Z", "baseRefName": "master",
              "commits": {{"nodes": [{{"commit": {{"statusCheckRollup": {rollup},
                                              "checkSuites": {suites}}}}}]}}
            }}"#
        )
    }

    #[test]
    fn a_required_check_that_has_not_started_is_pending() {
        let json = status_json(
            r#"{"contexts": {"nodes": [
              {"__typename": "CheckRun", "name": "build", "status": "COMPLETED",
               "conclusion": "SUCCESS", "startedAt": null, "detailsUrl": null,
               "checkSuite": {"workflowRun": null}}
            ]}}"#,
            "null",
        );
        let mut status: GqlStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status.base_branch(), Some("master"));
        status.add_required(&["build".to_string(), "Build (Pull Request)".to_string()]);
        let status = PrStatus::from(status);
        assert_eq!(status.status_check_rollup.len(), 2);
        let checks = crate::model::classify_checks(Mergeable::Other, &status.status_check_rollup);
        assert_eq!(checks.state, crate::model::CheckState::Pending);
    }

    #[test]
    fn a_required_check_is_added_to_a_commit_with_no_rollup() {
        let mut status: GqlStatus = serde_json::from_str(&status_json("null", "null")).unwrap();
        status.add_required(&["Build (Pull Request)".to_string()]);
        assert_eq!(PrStatus::from(status).status_check_rollup.len(), 1);
    }
}
