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

mod graphql;

/// The most PRs a refresh looks up one by one because the list no longer returns them.
const MAX_MISSING_LOOKUPS: usize = 10;

/// The branches asked for at a time. This is the most GitHub gives.
const BRANCHES_PER_PAGE: usize = 100;

/// The most pages of branches fetched.
const MAX_BRANCH_PAGES: usize = 5;

/// The users or teams asked for at a time. This is the most GitHub gives.
const NAMES_PER_PAGE: usize = 100;

/// The most pages of users or of teams fetched, so at most a thousand of each.
const MAX_NAME_PAGES: usize = 10;

/// The most pages of comments fetched for a PR, so at most [`graphql::MAX_PAGE`] times this.
const MAX_COMMENT_PAGES: usize = 20;

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
        let prs: Vec<graphql::GqlPr> = self.search(query, limit, graphql::Fields::List).await?;
        Ok(prs.into_iter().map(Pr::from).collect())
    }

    /// The PRs that `query` finds, up to one more than `limit`. It is one GraphQL search, a page
    /// of [`graphql::MAX_PAGE`] at a time for a longer list.
    async fn search<T: DeserializeOwned>(
        &self,
        query: &str,
        limit: u32,
        fields: graphql::Fields,
    ) -> Result<Vec<T>> {
        let wanted = limit.saturating_add(1) as usize;
        let graphql_query = graphql::search_query(fields);
        let search = graphql::search_string(query);
        let mut prs = Vec::new();
        let mut after: Option<String> = None;
        while prs.len() < wanted {
            let first = (wanted - prs.len()).min(graphql::MAX_PAGE as usize);
            let mut variables = vec![
                Variable::Typed("q", search.clone()),
                Variable::Typed("first", first.to_string()),
            ];
            if let Some(cursor) = &after {
                variables.push(Variable::Text("after", cursor));
            }
            let response: graphql::Response<graphql::SearchData<T>> =
                self.graphql(&graphql_query, &variables).await?;
            let page = response.data.search;
            let next = page
                .page_info
                .end_cursor
                .clone()
                .filter(|_| page.page_info.has_next_page);
            prs.extend(page.prs());
            match next {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
        prs.truncate(wanted);
        Ok(prs)
    }

    /// These PRs by number, in one request. One that isn't found is left out; if GitHub says
    /// anything is wrong with the request, it all fails.
    async fn lookup<T: DeserializeOwned>(
        &self,
        numbers: &[u64],
        fields: graphql::Fields,
    ) -> Result<Vec<T>> {
        let query = graphql::lookup_query(numbers, fields);
        let variables = [
            Variable::Typed("owner", "{owner}".to_string()),
            Variable::Typed("name", "{repo}".to_string()),
        ];
        let mut response: graphql::Response<graphql::LookupData<T>> =
            self.graphql(&query, &variables).await?;
        Ok(numbers
            .iter()
            .filter_map(|&number| response.data.take(number))
            .collect())
    }

    /// Runs a GraphQL query with `gh api graphql` and reads its `data`. Only queries: the text
    /// must start with `query`, so nothing here can send a mutation. A failure is `gh`'s output
    /// with a response's `errors` cut down to their messages.
    async fn graphql<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: &[Variable<'_>],
    ) -> Result<T> {
        if !query.trim_start().starts_with("query") {
            bail!("only GraphQL queries are sent");
        }
        let mut args = vec!["api".to_string(), "graphql".to_string()];
        args.extend(["-f".to_string(), format!("query={query}")]);
        for variable in variables {
            let (flag, name, value) = match variable {
                // `-F` fills in `{owner}` and `{repo}`, and reads a number as one.
                Variable::Typed(name, value) => ("-F", name, value.as_str()),
                // `-f` passes the value untouched, which a cursor needs.
                Variable::Text(name, value) => ("-f", name, *value),
            };
            args.extend([flag.to_string(), format!("{name}={value}")]);
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match self.exec_in(&args, true, true).await {
            Ok(stdout) => Ok(serde_json::from_slice(&stdout)?),
            Err(error) => match error.downcast::<GhError>() {
                Ok(mut failure) => {
                    failure.output = graphql::error_message(&failure.output);
                    // The command would be the whole query.
                    failure.command = "gh api graphql".to_string();
                    Err(failure.into())
                }
                Err(error) => Err(error),
            },
        }
    }

    /// The status of each PR the query lists, and of up to [`MAX_MISSING_LOOKUPS`] of the
    /// `candidates` it doesn't, such as a PR merged by someone else since the list loaded, which
    /// are looked up together. It uses what it got even if some of it failed, and says why in
    /// `failure`. A failed lookup fails for all of the candidates.
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
                update.failure = Some(failure_text(&e));
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
        if missing.is_empty() {
            return update;
        }
        match self
            .lookup::<graphql::GqlStatus>(&missing, graphql::Fields::Status)
            .await
        {
            Ok(found) => update
                .statuses
                .extend(found.into_iter().map(PrStatus::from)),
            Err(e) => update.failure = Some(failure_text(&e)),
        }
        update
    }

    async fn list_statuses(&self, query: &str, limit: u32) -> Result<Vec<PrStatus>> {
        let statuses: Vec<graphql::GqlStatus> =
            self.search(query, limit, graphql::Fields::Status).await?;
        Ok(statuses.into_iter().map(PrStatus::from).collect())
    }

    /// One PR with the same fields as the list, to refresh its row.
    pub async fn pr(&self, number: u64) -> Result<Pr> {
        let found: Vec<graphql::GqlPr> = self.lookup(&[number], graphql::Fields::List).await?;
        match found.into_iter().next() {
            Some(pr) => Ok(pr.into()),
            None => bail!("PR #{number} was not found"),
        }
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

    /// The names of the repository's labels, for completing `label:`.
    pub async fn labels(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Label {
            name: String,
        }
        let stdout = self
            .run(&["label", "list", "--json", "name", "--limit", "1000"])
            .await?;
        let labels: Vec<Label> = serde_json::from_slice(&stdout)?;
        Ok(labels.into_iter().map(|label| label.name).collect())
    }

    /// The names of the repository's branches, for completing `base:` and `head:`, up to
    /// [`MAX_BRANCH_PAGES`] pages of [`BRANCHES_PER_PAGE`]. A repository with more has the rest
    /// left out, which only means fewer suggestions.
    pub async fn branches(&self) -> Result<Vec<String>> {
        let mut branches = Vec::new();
        for page in 1..=MAX_BRANCH_PAGES {
            let path = format!(
                "repos/{{owner}}/{{repo}}/branches?per_page={BRANCHES_PER_PAGE}&page={page}"
            );
            let stdout = self
                .exec_in(&["api", &path, "--jq", ".[].name"], true, true)
                .await?;
            let before = branches.len();
            branches.extend(String::from_utf8_lossy(&stdout).lines().map(str::to_string));
            if branches.len() - before < BRANCHES_PER_PAGE {
                break;
            }
        }
        Ok(branches)
    }

    /// The logins of the users who can be assigned to the repository's PRs, for completing the
    /// qualifiers that name a reviewer.
    pub async fn users(&self) -> Result<Vec<String>> {
        let path = "repos/{owner}/{repo}/assignees";
        self.pages(path, ".[].login", MAX_NAME_PAGES).await
    }

    /// The repository owner's teams as `org/slug`, for completing `team-review-requested:`. Only
    /// what the token may see: without the `read:org` scope this fails. The list gives no
    /// organisation, so it is read from each team's page.
    pub async fn teams(&self) -> Result<Vec<String>> {
        let path = "orgs/{owner}/teams";
        let jq = r#".[] | .html_url | capture("/orgs/(?<org>[^/]+)/teams/(?<slug>[^/]+)") | .org + "/" + .slug"#;
        self.pages(path, jq, MAX_NAME_PAGES).await
    }

    /// One name for each line that `jq` makes of each page of `path`, up to `max_pages` of them.
    /// The first page says whether there are more, and then the rest are fetched together, as a
    /// long list fetched a page at a time takes too long. A later page that fails is left out,
    /// which only means fewer suggestions.
    async fn pages(&self, path: &str, jq: &str, max_pages: usize) -> Result<Vec<String>> {
        let page = async |page: usize| -> Result<Vec<String>> {
            let path = format!("{path}?per_page={NAMES_PER_PAGE}&page={page}");
            let stdout = self
                .exec_in(&["api", &path, "--jq", jq], true, true)
                .await?;
            Ok(String::from_utf8_lossy(&stdout)
                .lines()
                .map(str::to_string)
                .collect())
        };
        let mut names = page(1).await?;
        if names.len() == NAMES_PER_PAGE {
            let rest = futures::future::join_all((2..=max_pages).map(page)).await;
            names.extend(rest.into_iter().filter_map(Result::ok).flatten());
        }
        Ok(names)
    }

    /// The details of a PR, with its comments a page at a time, up to [`MAX_COMMENT_PAGES`] of
    /// them. A PR with more has the newest left out.
    pub async fn pr_details(&self, number: u64) -> Result<PrDetails> {
        let mut details: Option<PrDetails> = None;
        let mut after: Option<String> = None;
        for _ in 0..MAX_COMMENT_PAGES {
            let mut variables = vec![
                Variable::Typed("owner", "{owner}".to_string()),
                Variable::Typed("name", "{repo}".to_string()),
                Variable::Typed("number", number.to_string()),
                // The reviews and threads come with the first page only.
                Variable::Typed("withReviews", details.is_none().to_string()),
            ];
            if let Some(cursor) = &after {
                variables.push(Variable::Text("after", cursor));
            }
            let response: graphql::Response<graphql::DetailsData> =
                self.graphql(graphql::DETAILS_QUERY, &variables).await?;
            let (page, next) = response.data.into_page();
            match &mut details {
                Some(details) => details.comments.extend(page.comments),
                None => details = Some(page),
            }
            match next {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
        // The loop runs at least once, and fails rather than leaving `details` empty.
        let mut details = details.ok_or_else(|| eyre!("no details were fetched"))?;
        graphql::sort_comments(&mut details.comments);
        Ok(details)
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
        self.exec_in(args, kill_on_drop, false).await
    }

    /// `gh api` has no `--repo`, so `{owner}` and `{repo}` in its path are those of the repository
    /// in `GH_REPO` instead, which is set when there is a `--repo`.
    async fn exec_in(&self, args: &[&str], kill_on_drop: bool, api: bool) -> Result<Vec<u8>> {
        let mut args: Vec<&str> = args.to_vec();
        let mut command = Command::new("gh");
        if let Some(repo) = &self.repo {
            if api {
                command.env("GH_REPO", repo);
            } else {
                args.extend(["--repo", repo]);
            }
        }
        let output = command
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

/// What to keep of a failed call: for a `gh` command, what `gh` said, without the command. The
/// panes show its first line; the rest is there for a view of the whole message.
pub fn failure_text(error: &color_eyre::Report) -> String {
    match error.downcast_ref::<GhError>() {
        Some(gh) => gh.output.clone(),
        None => error.to_string(),
    }
}

/// A variable of a GraphQL query.
enum Variable<'a> {
    /// Passed with `-F`: a `{owner}` or `{repo}` in it is filled in, and a number is sent as one.
    Typed(&'a str, String),
    /// Passed with `-f`, as the text it is.
    Text(&'a str, &'a str),
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
    /// The PR's page on GitHub.
    pub url: String,
    /// When the PR last changed, as GitHub reports it. A change means its details and diff may
    /// have changed too.
    pub updated_at: String,
    pub author: Author,
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
#[derive(Debug)]
#[cfg_attr(test, derive(Deserialize))]
#[cfg_attr(test, serde(rename_all = "camelCase"))]
pub struct PrDetails {
    pub head_ref_name: String,
    pub body: Option<String>,
    pub comments: Vec<Comment>,
    pub additions: u64,
    pub deletions: u64,
    /// Who has reviewed the PR and who has been asked to.
    #[cfg_attr(test, serde(default))]
    pub approvers: Approvers,
}

/// Who a review was asked of, or who gave one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Deserialize))]
pub enum Reviewer {
    /// A person, or a bot, by login; a bot's has the `app/` prefix.
    Person(String),
    /// A team as `org/slug`.
    Team(String),
    /// A reviewer GitHub didn't say, such as a team the account can't see.
    Hidden,
}

/// A person's latest review that said something: approved, asked for changes, or was dismissed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Deserialize))]
pub struct Approval {
    pub by: Reviewer,
    pub verdict: Verdict,
    /// Whether the PR has new commits since the review.
    pub stale: bool,
    /// The teams the review was given for, as `org/slug`. It is how a team's request is met.
    #[cfg_attr(test, serde(default))]
    pub for_teams: Vec<String>,
}

/// Someone a review is still asked of.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Deserialize))]
pub struct ReviewRequest {
    pub of: Reviewer,
    /// Whether it was asked for because of CODEOWNERS.
    pub code_owner: bool,
}

/// The reviews of a PR, as the approvers pane shows them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Deserialize))]
#[cfg_attr(test, serde(default))]
pub struct Approvers {
    pub approvals: Vec<Approval>,
    pub requests: Vec<ReviewRequest>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

#[derive(Debug)]
#[cfg_attr(test, derive(Deserialize))]
#[cfg_attr(test, serde(rename_all = "camelCase"))]
pub struct Comment {
    pub author: Option<Author>,
    /// When it was written; for a review, when it was submitted. The comments are in this order.
    pub created_at: String,
    pub body: Option<String>,
    /// The comment's page in the web UI. For a thread, its first comment's.
    pub url: Option<String>,
    #[cfg_attr(test, serde(default))]
    pub kind: CommentKind,
    /// The comments after the first in a thread.
    #[cfg_attr(test, serde(default))]
    pub replies: Vec<Reply>,
}

/// What a comment is: one on the PR itself, a review's own comment, or a review thread on a line
/// of the diff.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Deserialize))]
pub enum CommentKind {
    #[default]
    Comment,
    Review(Verdict),
    Thread(Thread),
}

/// How a review ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
}

/// A review thread, whose first comment is the `Comment` it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Deserialize))]
pub struct Thread {
    pub path: String,
    /// The line it is on, or the one it was on if the code has changed since. `None` for a
    /// comment on the whole file.
    pub line: Option<u64>,
    pub resolved: bool,
    /// Whether the code it is on has changed since.
    pub outdated: bool,
}

/// A comment in a thread after the first.
#[derive(Debug)]
#[cfg_attr(test, derive(Deserialize))]
#[cfg_attr(test, serde(rename_all = "camelCase"))]
pub struct Reply {
    pub author: Option<Author>,
    pub created_at: String,
    pub body: Option<String>,
}

impl Comment {
    /// What to say about the comment in its header, after who wrote it: nothing for a comment on
    /// the PR, how a review ended, and where a thread is and how it stands.
    pub fn tags(&self) -> Vec<String> {
        match &self.kind {
            CommentKind::Comment => Vec::new(),
            CommentKind::Review(verdict) => vec![
                match verdict {
                    Verdict::Approved => "approved",
                    Verdict::ChangesRequested => "changes requested",
                    Verdict::Commented => "reviewed",
                    Verdict::Dismissed => "dismissed review",
                }
                .to_string(),
            ],
            CommentKind::Thread(thread) => {
                let mut tags = vec![match thread.line {
                    Some(line) => format!("{}:{line}", thread.path),
                    None => thread.path.clone(),
                }];
                match self.replies.len() {
                    0 => {}
                    1 => tags.push("1 reply".to_string()),
                    n => tags.push(format!("{n} replies")),
                }
                if thread.resolved {
                    tags.push("resolved".to_string());
                }
                if thread.outdated {
                    tags.push("outdated".to_string());
                }
                tags
            }
        }
    }
}

/// Who wrote a PR or a comment. A bot's login has the `app/` prefix that `author:` takes in a
/// query.
#[derive(Debug, Clone, Deserialize)]
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
        "url": "https://example.test/pull/12",
        "author": {"login": "alice"},
        "updatedAt": "2026-01-02T03:04:06Z"
      },
      {
        "number": 13,
        "title": "Merged one",
        "state": "MERGED",
        "statusCheckRollup": [],
        "mergeable": "UNKNOWN",
        "reviewDecision": "APPROVED",
        "url": "https://example.test/pull/13",
        "author": {"is_bot": true, "login": "app/renovate"},
        "updatedAt": "2026-01-02T03:04:07Z"
      }
    ]"#;

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
          "url": "https://example.test/pull/1",
          "author": {"login": "alice"},
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

    #[tokio::test]
    async fn only_queries_are_sent_as_graphql() {
        let gh = Gh::new(None, false);
        for text in [
            "mutation { addStar(input: {starrableId: \"x\"}) { clientMutationId } }",
            "  mutation($id: ID!) { x }",
            "{ viewer { login } }",
        ] {
            let error = gh
                .graphql::<serde_json::Value>(text, &[])
                .await
                .unwrap_err();
            assert_eq!(error.to_string(), "only GraphQL queries are sent", "{text}");
        }
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
