//! The search queries submitted before, walked with Up and Down in the search box and kept between
//! runs.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;

/// The most queries kept, unless told otherwise.
pub const DEFAULT_LIMIT: usize = 5000;

/// Where the history file lives: the platform's state directory, or its data directory where
/// there is no state directory (macOS and Windows). `None` when there is no home directory.
pub fn default_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "prism")?;
    let dir = dirs.state_dir().unwrap_or_else(|| dirs.data_dir());
    Some(dir.join("history"))
}

#[derive(Default)]
pub struct History {
    /// The queries, oldest first, each once.
    entries: Vec<String>,
    /// The queries submitted in this run, oldest first, each once. They are what gets added to
    /// the file, which another run may have written to since it was read.
    added: Vec<String>,
    /// The entry shown by the walk, or `None` when the walk is not under way.
    cursor: Option<usize>,
    /// What was in the box when the walk began.
    stash: String,
    /// Where the history is kept, or `None` to not keep it.
    path: Option<PathBuf>,
    limit: usize,
}

/// Moves `query` to the end of `queries`, or adds it there.
fn move_to_end(queries: &mut Vec<String>, query: String) {
    queries.retain(|q| *q != query);
    queries.push(query);
}

/// Drops the oldest queries so that no more than `limit` are left.
fn cap(queries: &mut Vec<String>, limit: usize) {
    let excess = queries.len().saturating_sub(limit);
    queries.drain(..excess);
}

/// Reads one query for each non-empty line, each once, the last place of a repeat winning.
fn read(path: &Path) -> Vec<String> {
    let mut queries = Vec::new();
    for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
        if !line.trim().is_empty() {
            move_to_end(&mut queries, line.to_string());
        }
    }
    queries
}

impl History {
    /// The history in `path`. A missing or unreadable file is an empty history, and a `path` of
    /// `None` is one that is never written.
    pub fn load(path: Option<PathBuf>, limit: usize) -> Self {
        let mut entries = path.as_deref().map(read).unwrap_or_default();
        cap(&mut entries, limit);
        Self {
            entries,
            path,
            limit,
            ..Self::default()
        }
    }

    /// Notes a submitted query as the newest, in place of any earlier copy. Empty ones are not
    /// kept.
    pub fn record(&mut self, query: &str) {
        self.cursor = None;
        // A line holds one query.
        let query = query.replace(['\n', '\r'], " ");
        if query.trim().is_empty() {
            return;
        }
        move_to_end(&mut self.entries, query.clone());
        cap(&mut self.entries, self.limit);
        move_to_end(&mut self.added, query);
    }

    /// The next older query than the one shown, for Up. The first one stashes `current`. `None`
    /// when there is nothing older, in which case the box stays as it is.
    ///
    /// A walk goes on only while the box still holds what the walk put there: editing it, or
    /// leaving and coming back, starts the walk again from the text that is there.
    pub fn previous(&mut self, current: &str) -> Option<&str> {
        let cursor = match self.walking(current) {
            Some(0) => return None,
            Some(shown) => shown - 1,
            // A walk begun on a query that is already the newest skips it, as it would change
            // nothing.
            None => {
                let cursor = self.entries.iter().rposition(|q| q != current)?;
                self.stash = current.to_string();
                cursor
            }
        };
        self.cursor = Some(cursor);
        Some(&self.entries[cursor])
    }

    /// The next newer query than the one shown, for Down, and after the newest what was stashed.
    /// `None` when no walk is under way.
    pub fn next(&mut self, current: &str) -> Option<&str> {
        let shown = self.walking(current)?;
        if shown + 1 < self.entries.len() {
            self.cursor = Some(shown + 1);
            Some(&self.entries[shown + 1])
        } else {
            self.cursor = None;
            Some(&self.stash)
        }
    }

    /// The entry shown by the walk, if one is under way and `current` is still it.
    fn walking(&self, current: &str) -> Option<usize> {
        self.cursor
            .filter(|&cursor| self.entries.get(cursor).is_some_and(|q| q == current))
    }

    /// Writes the history, with this run's queries added to what is in the file now rather than
    /// to what was read at the start, so a run that ended in the meantime keeps its queries. A
    /// failure to write is the history not being kept, and nothing more.
    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        if self.added.is_empty() {
            return;
        }
        let mut queries = read(path);
        for query in &self.added {
            move_to_end(&mut queries, query.clone());
        }
        cap(&mut queries, self.limit);
        let _ = write(path, &queries);
    }
}

/// Writes `queries` to a temporary file and renames it over `path`, so a reader never sees half
/// of it.
fn write(path: &Path, queries: &[String]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut text = queries.join("\n");
    text.push('\n');
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A history that was never written, holding `queries`.
    fn history_of(queries: &[&str]) -> History {
        let mut history = History::load(None, 100);
        for query in queries {
            history.record(query);
        }
        history
    }

    fn lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn up_walks_back_and_down_walks_forward_to_what_was_typed() {
        let mut history = history_of(&["a", "b", "c"]);
        assert_eq!(history.previous("typed"), Some("c"));
        assert_eq!(history.previous("c"), Some("b"));
        assert_eq!(history.previous("b"), Some("a"));
        // The oldest stays put.
        assert_eq!(history.previous("a"), None);
        assert_eq!(history.next("a"), Some("b"));
        assert_eq!(history.next("b"), Some("c"));
        assert_eq!(history.next("c"), Some("typed"));
        // The walk is over.
        assert_eq!(history.next("typed"), None);
    }

    #[test]
    fn a_walk_skips_the_query_already_in_the_box() {
        let mut history = history_of(&["a", "b"]);
        assert_eq!(history.previous("b"), Some("a"));
        assert_eq!(history.next("a"), Some("b"));
        // The only entry is what is there, so there is nowhere to go.
        assert_eq!(history_of(&["a"]).previous("a"), None);
    }

    #[test]
    fn an_empty_history_walks_nowhere() {
        let mut history = history_of(&[]);
        assert_eq!(history.previous("typed"), None);
        assert_eq!(history.next("typed"), None);
    }

    #[test]
    fn editing_a_recalled_query_starts_the_walk_again_from_the_edit() {
        let mut history = history_of(&["a", "b"]);
        assert_eq!(history.previous("typed"), Some("b"));
        assert_eq!(history.previous("b edited"), Some("b"));
        assert_eq!(history.next("b"), Some("b edited"));
    }

    #[test]
    fn a_repeated_query_moves_to_the_newest_place() {
        let history = history_of(&["a", "b", "c", "a"]);
        assert_eq!(history.entries, ["b", "c", "a"]);
        assert_eq!(history.added, ["b", "c", "a"]);
    }

    #[test]
    fn empty_queries_are_not_kept() {
        let history = history_of(&["", "   ", "a"]);
        assert_eq!(history.entries, ["a"]);
    }

    #[test]
    fn the_oldest_go_when_over_the_limit() {
        let mut history = History::load(None, 2);
        for query in ["a", "b", "c"] {
            history.record(query);
        }
        assert_eq!(history.entries, ["b", "c"]);
    }

    #[test]
    fn load_drops_repeats_and_blanks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history");
        std::fs::write(&path, "a\n\nb\na\n").unwrap();
        assert_eq!(History::load(Some(path), 100).entries, ["b", "a"]);
    }

    #[test]
    fn a_missing_file_is_an_empty_history_and_save_makes_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state").join("history");
        let mut history = History::load(Some(path.clone()), 100);
        assert!(history.entries.is_empty());
        history.record("a");
        history.save();
        assert_eq!(lines(&path), ["a"]);
    }

    #[test]
    fn save_keeps_what_another_run_wrote_since_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history");
        std::fs::write(&path, "a\nb\n").unwrap();
        let mut ours = History::load(Some(path.clone()), 100);
        let mut theirs = History::load(Some(path.clone()), 100);
        theirs.record("from other");
        theirs.save();
        ours.record("a");
        ours.record("ours");
        ours.save();
        assert_eq!(lines(&path), ["b", "from other", "a", "ours"]);
    }

    #[test]
    fn save_caps_the_file_and_leaves_no_temporary_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history");
        std::fs::write(&path, "a\nb\n").unwrap();
        let mut history = History::load(Some(path.clone()), 2);
        history.record("c");
        history.save();
        assert_eq!(lines(&path), ["b", "c"]);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn save_without_a_path_or_in_an_unwritable_place_does_nothing() {
        let history = history_of(&["a"]);
        history.save();
        let dir = tempfile::tempdir().unwrap();
        // A file where the directory should be.
        let file = dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        let mut history = History::load(Some(file.join("history")), 100);
        history.record("a");
        history.save();
        // Nothing was written over the file, and nothing was made beside it.
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
