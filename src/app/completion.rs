//! The list of suggestions under the search box: which one is highlighted, and what the keys do
//! while it is open. What is suggested is worked out in `query`.

use tui_input::Input;

use super::{App, Typing};
use crate::query::{self, Candidates, Completion};

/// The highlight and the dismissal of the list for one state of the search box. A new state, from
/// any change to the text or the cursor, starts it afresh, so Esc closes the list only until the
/// next key.
#[derive(Default)]
pub struct CompletionView {
    /// The text and the cursor this is about.
    key: (String, usize),
    /// The suggestion chosen, if one has been: the list opens with none.
    selected: Option<usize>,
    dismissed: bool,
}

impl App {
    /// The suggestions for the word at the cursor, and the one chosen, if any. `None` while the box
    /// is not being edited, before its text has been changed (so that opening the box and then
    /// Esc is not met by a list), when nothing fits, or when the list was closed with Esc.
    pub fn completion(&self) -> Option<(Completion, Option<usize>)> {
        if self.typing != Typing::Search || self.query.value() == self.submitted_query {
            return None;
        }
        let view = &self.completion_view;
        let key = (self.query.value().to_string(), self.query.cursor());
        let current = view.key == key;
        if current && view.dismissed {
            return None;
        }
        let authors = self.authors();
        let have = Candidates {
            authors: &authors,
            labels: self.names.labels(),
            branches: self.names.branches(),
            users: self.names.users(),
            teams: self.names.teams(),
        };
        let completion = query::complete(&key.0, key.1, &have)?;
        let selected = view.selected.filter(|_| current);
        let selected = selected.map(|s| s.min(completion.suggestions.len() - 1));
        Some((completion, selected))
    }

    /// The authors of the PRs listed, each once.
    fn authors(&self) -> Vec<String> {
        let mut authors: Vec<String> = self
            .rows
            .iter()
            .map(|row| row.pr.author.login.clone())
            .collect();
        authors.sort();
        authors.dedup();
        authors
    }

    /// The view of the box as it is now, started afresh if the box has changed since.
    fn view(&mut self) -> &mut CompletionView {
        let key = (self.query.value().to_string(), self.query.cursor());
        if self.completion_view.key != key {
            self.completion_view = CompletionView {
                key,
                ..CompletionView::default()
            };
        }
        &mut self.completion_view
    }

    /// Enter: puts the suggestion that was chosen in the query, which closes the list unless what
    /// it was followed by has suggestions of its own. Returns whether one was chosen.
    pub(super) fn accept_completion(&mut self) -> bool {
        let Some((completion, Some(selected))) = self.completion() else {
            return false;
        };
        let (accepted, cursor) = completion.accept(self.query.value(), selected);
        self.query = Input::new(accepted).with_cursor(cursor);
        true
    }

    /// Tab and Down, or Shift-Tab and Up with `step` of -1: chooses the next suggestion, round the
    /// ends. The first press chooses the first, whichever way it goes. Returns whether the list
    /// was open.
    pub(super) fn move_completion(&mut self, step: isize) -> bool {
        let Some((completion, selected)) = self.completion() else {
            return false;
        };
        let count = completion.suggestions.len();
        let moved = match selected {
            Some(selected) => (selected + count).wrapping_add_signed(step) % count,
            None => 0,
        };
        self.view().selected = Some(moved);
        true
    }

    /// Esc: closes the list until the box next changes. Returns whether it was open.
    pub(super) fn dismiss_completion(&mut self) -> bool {
        if self.completion().is_none() {
            return false;
        }
        self.view().dismissed = true;
        true
    }

    /// Closes the list for the text that is in the box now, such as one just recalled from the
    /// history, so that Up and Down carry on through the history.
    pub(super) fn hide_completion(&mut self) {
        self.view().dismissed = true;
    }
}
