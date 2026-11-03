use clap::{Parser, ValueEnum};
use regex::Regex;
use serde::Deserialize;

use crate::config::Config;
use crate::model;

/// Review, approve and merge open PRs.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Args {
    /// Repository as OWNER/NAME (default: the current directory's repository).
    #[arg(long)]
    pub repo: Option<String>,

    /// Only list PRs by this GitHub author, e.g. app/renovate (default: any author).
    #[arg(long, env = "PRISM_AUTHOR")]
    pub author: Option<String>,

    /// Extra GitHub search terms, e.g. 'label:non-prod -label:major'.
    #[arg(long)]
    pub search: Option<String>,

    /// Regex whose named groups become columns (default: $PRISM_COLUMN_REGEX), e.g.
    /// 'Bump (?<container>.+?)(?: from \S+)? to \S+$'.
    #[arg(long, env = "PRISM_COLUMN_REGEX", value_parser = model::parse_column_regex)]
    pub column_regex: Option<Regex>,

    /// Most PRs to fetch at once. GitHub times out on large repos if this is too high.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,

    /// How to merge approved PRs (default: squash).
    #[arg(long, value_enum)]
    pub merge_method: Option<MergeMethod>,

    /// Most search queries to remember between runs.
    #[arg(long, env = "PRISM_HISTORY_LIMIT")]
    pub history_limit: Option<usize>,

    /// Don't capture the mouse, so the terminal's own text selection works.
    #[arg(long)]
    pub no_mouse: bool,

    /// Walk through the flow without approving or merging anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
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

/// What the program runs with: each setting from the flag or environment variable if it was given,
/// else from the config file, else its default.
#[derive(Debug)]
pub struct Settings {
    pub repo: Option<String>,
    pub author: Option<String>,
    pub search: String,
    pub column_regex: Option<Regex>,
    pub limit: u32,
    pub merge_method: MergeMethod,
    pub mouse: bool,
    pub history_limit: usize,
    pub dry_run: bool,
}

impl Args {
    /// The settings, with `config` filling in what the arguments left out. A config file value
    /// that a flag would refuse is refused too.
    pub fn settings(self, config: Config) -> Result<Settings, String> {
        let column_regex = match (self.column_regex, config.column_regex) {
            (Some(regex), _) => Some(regex),
            (None, Some(pattern)) => Some(
                model::parse_column_regex(&pattern)
                    .map_err(|error| format!("config column_regex: {error}"))?,
            ),
            (None, None) => None,
        };
        if config.limit == Some(0) {
            return Err("config limit must be at least 1".to_string());
        }
        Ok(Settings {
            repo: self.repo,
            author: self.author.or(config.author),
            search: self.search.or(config.search).unwrap_or_default(),
            column_regex,
            limit: self.limit.or(config.limit).unwrap_or(30),
            merge_method: self
                .merge_method
                .or(config.merge_method)
                .unwrap_or(MergeMethod::Squash),
            // `--no-mouse` can only turn the mouse off, so the config decides without it.
            mouse: !self.no_mouse && config.mouse.unwrap_or(true),
            history_limit: self
                .history_limit
                .or(config.history_limit)
                .unwrap_or(crate::history::DEFAULT_LIMIT),
            dry_run: self.dry_run,
        })
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

    fn settings(flags: &[&str], config: Config) -> Result<Settings, String> {
        let args = Args::try_parse_from(std::iter::once("prism").chain(flags.iter().copied()));
        args.unwrap().settings(config)
    }

    fn config(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn defaults_apply_with_no_flags_and_no_config() {
        let s = settings(&[], Config::default()).unwrap();
        assert_eq!(s.search, "");
        assert_eq!(s.limit, 30);
        assert!(matches!(s.merge_method, MergeMethod::Squash));
        assert!(s.mouse);
        assert_eq!(s.history_limit, crate::history::DEFAULT_LIMIT);
        assert!(s.author.is_none() && s.column_regex.is_none());
    }

    #[test]
    fn config_overrides_the_defaults() {
        let s = settings(
            &[],
            config(
                "author = \"a\"\nsearch = \"label:x\"\nlimit = 5\nmerge_method = \"merge\"\n\
                 mouse = false\nhistory_limit = 9\ncolumn_regex = \"(?<p>x)\"",
            ),
        )
        .unwrap();
        assert_eq!(s.author.as_deref(), Some("a"));
        assert_eq!(s.search, "label:x");
        assert_eq!(s.limit, 5);
        assert!(matches!(s.merge_method, MergeMethod::Merge));
        assert!(!s.mouse);
        assert_eq!(s.history_limit, 9);
        assert!(s.column_regex.is_some());
    }

    #[test]
    fn flags_win_over_config() {
        let s = settings(
            &[
                "--author",
                "f",
                "--search",
                "s",
                "--limit",
                "7",
                "--merge-method",
                "rebase",
                "--history-limit",
                "3",
                "--column-regex",
                "(?<q>y)",
            ],
            config(
                "author = \"a\"\nsearch = \"label:x\"\nlimit = 5\nmerge_method = \"merge\"\n\
                 history_limit = 9\ncolumn_regex = \"(?<p>x)\"",
            ),
        )
        .unwrap();
        assert_eq!(s.author.as_deref(), Some("f"));
        assert_eq!(s.search, "s");
        assert_eq!(s.limit, 7);
        assert!(matches!(s.merge_method, MergeMethod::Rebase));
        assert_eq!(s.history_limit, 3);
        let regex = s.column_regex.unwrap();
        assert_eq!(model::column_names(&regex), ["q"]);
    }

    #[test]
    fn no_mouse_wins_over_config_but_cannot_turn_the_mouse_on() {
        let off = settings(&["--no-mouse"], config("mouse = true")).unwrap();
        assert!(!off.mouse);
        let on = settings(&[], config("mouse = true")).unwrap();
        assert!(on.mouse);
    }

    #[test]
    fn a_config_value_a_flag_would_refuse_is_refused() {
        let error = settings(&[], config("limit = 0")).unwrap_err();
        assert!(error.contains("limit"), "{error}");
        let error = settings(&[], config("column_regex = \"(\"")).unwrap_err();
        assert!(error.contains("column_regex"), "{error}");
        let error = settings(&[], config("column_regex = \"no groups\"")).unwrap_err();
        assert!(error.contains("column_regex"), "{error}");
    }

    #[test]
    fn a_flag_covers_a_bad_config_regex() {
        let s = settings(
            &["--column-regex", "(?<q>y)"],
            config("column_regex = \"(\""),
        )
        .unwrap();
        assert!(s.column_regex.is_some());
    }

    #[test]
    fn title_shows_repo_and_dry_run() {
        assert_eq!(title(None, false), "PRs");
        assert_eq!(title(Some("o/r"), false), "PRs (o/r)");
        assert_eq!(title(Some("o/r"), true), "PRs (o/r) [DRY RUN]");
    }
}
