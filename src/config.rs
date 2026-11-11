//! The config file: defaults for the flags, which the flags and the environment win over.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::Deserialize;

use crate::cli::MergeMethod;

/// Every setting is optional, and a key that isn't one of these is an error, so a typo doesn't
/// pass for a setting that was applied.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Like `--author`.
    pub author: Option<String>,
    /// Like `--search`.
    pub search: Option<String>,
    /// Like `--column-regex`.
    pub column_regex: Option<String>,
    /// Like `--limit`.
    pub limit: Option<u32>,
    /// Like `--merge-method`: `squash`, `merge` or `rebase`.
    pub merge_method: Option<MergeMethod>,
    /// `false` is like `--no-mouse`.
    pub mouse: Option<bool>,
    /// Like `--history-limit`.
    pub history_limit: Option<usize>,
}

/// Where the config file lives: `config.toml` in the platform's config directory. `None` when
/// there is no home directory.
pub fn default_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "prism")?;
    Some(dirs.config_dir().join("config.toml"))
}

impl Config {
    /// The config in `path`. A missing file, or a `path` of `None`, is an empty config. A file
    /// that can't be read or understood is an error saying which, as running with settings the
    /// user thinks are on but aren't is worse than not starting.
    pub fn load(path: Option<&Path>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("can't read {}: {error}", path.display())),
        };
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(text: &str) -> Result<Config, String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        Config::load(Some(&path))
    }

    #[test]
    fn reads_every_setting() {
        let config = load(
            r#"
author = "app/renovate"
search = "label:deps"
column_regex = "(?<a>x)"
limit = 10
merge_method = "rebase"
mouse = false
history_limit = 20
"#,
        )
        .unwrap();
        assert_eq!(config.author.as_deref(), Some("app/renovate"));
        assert_eq!(config.search.as_deref(), Some("label:deps"));
        assert_eq!(config.column_regex.as_deref(), Some("(?<a>x)"));
        assert_eq!(config.limit, Some(10));
        assert!(matches!(config.merge_method, Some(MergeMethod::Rebase)));
        assert_eq!(config.mouse, Some(false));
        assert_eq!(config.history_limit, Some(20));
    }

    #[test]
    fn an_empty_file_sets_nothing() {
        // Naming every field, so that a new one has to be checked here too.
        let Config {
            author,
            search,
            column_regex,
            limit,
            merge_method,
            mouse,
            history_limit,
        } = load("").unwrap();
        assert!(author.is_none() && search.is_none() && column_regex.is_none());
        assert!(limit.is_none() && merge_method.is_none());
        assert!(mouse.is_none() && history_limit.is_none());
    }

    #[test]
    fn a_missing_file_or_no_path_is_an_empty_config() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        assert!(Config::load(Some(&missing)).unwrap().limit.is_none());
        assert!(Config::load(None).unwrap().limit.is_none());
    }

    #[test]
    fn an_invalid_file_is_an_error_naming_the_file() {
        for text in [
            "limit = ",
            "limit = \"ten\"",
            "merge_method = \"fast\"",
            "colour = true",
        ] {
            let error = load(text).unwrap_err();
            assert!(error.contains("config.toml"), "{text}: {error}");
        }
    }

    #[test]
    fn an_unreadable_file_is_an_error() {
        // A directory where the file should be.
        let dir = tempfile::tempdir().unwrap();
        let error = Config::load(Some(dir.path())).unwrap_err();
        assert!(error.starts_with("can't read"), "{error}");
    }
}
