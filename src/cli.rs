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
