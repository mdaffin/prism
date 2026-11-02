use clap::{Parser, ValueEnum};
use regex::Regex;

use crate::model;

/// Review, approve and merge open PRs.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// Repository as OWNER/NAME (default: the current directory's repository).
    #[arg(long)]
    pub repo: Option<String>,

    /// Only list PRs by this GitHub author, e.g. app/renovate (default: any author).
    #[arg(long, env = "GH_PRUI_AUTHOR")]
    pub author: Option<String>,

    /// Extra GitHub search terms, e.g. 'label:non-prod -label:major'.
    #[arg(long, default_value = "")]
    pub search: String,

    /// Regex whose named groups become columns (default: $GH_PRUI_COLUMN_REGEX), e.g.
    /// 'Bump (?<container>.+?)(?: from \S+)? to \S+$'.
    #[arg(long, env = "GH_PRUI_COLUMN_REGEX", value_parser = model::parse_column_regex)]
    pub column_regex: Option<Regex>,

    /// Most PRs to fetch at once. GitHub times out on large repos if this is too high.
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: u32,

    /// How to merge approved PRs.
    #[arg(long, value_enum, default_value_t = MergeMethod::Squash)]
    pub merge_method: MergeMethod,

    /// Don't capture the mouse, so the terminal's own text selection works.
    #[arg(long)]
    pub no_mouse: bool,

    /// Walk through the flow without approving or merging anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum MergeMethod {
    Squash,
    Merge,
    Rebase,
}

impl MergeMethod {
    /// The `gh pr merge` flag for this method.
    pub fn flag(self) -> &'static str {
        match self {
            Self::Squash => "--squash",
            Self::Merge => "--merge",
            Self::Rebase => "--rebase",
        }
    }
}

/// `PRs (<repo>)`, plus ` [DRY RUN]`. The query isn't included: it can change, and the search
/// box shows it.
pub fn title(repo: Option<&str>, dry_run: bool) -> String {
    let mut title = String::from("PRs");
    if let Some(repo) = repo {
        title.push_str(&format!(" ({repo})"));
    }
    if dry_run {
        title.push_str(" [DRY RUN]");
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_shows_repo_and_dry_run() {
        assert_eq!(title(None, false), "PRs");
        assert_eq!(title(Some("o/r"), false), "PRs (o/r)");
        assert_eq!(title(Some("o/r"), true), "PRs (o/r) [DRY RUN]");
    }
}
