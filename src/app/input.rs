//! Keys and mouse clicks, routed by what the app is showing.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;
use tui_input::backend::crossterm::EventHandler;
use tui_input::{Input, InputRequest};
use unicode_width::UnicodeWidthChar;

use super::{App, Mode, Pane, Typing};
use crate::batch::on_confirm_key;

/// The columns that a key press or a notch of the wheel scrolls a pane sideways.
const SIDEWAYS_COLUMNS: isize = 8;

/// The index of the character of `text` drawn at `column`, counted in display columns from its
/// start, or its length when `column` is past the end.
fn char_at_column(text: &str, column: usize) -> usize {
    let mut x = 0;
    for (index, c) in text.chars().enumerate() {
        x += c.width().unwrap_or(0);
        if column < x {
            return index;
        }
    }
    text.chars().count()
}

impl App {
    /// While a dialog is open only a click on one of its buttons does anything. While a box is being
    /// edited only a click does anything: on a box it moves the cursor or switches to the other
    /// box, and anywhere else it leaves the box, keeping its text, and then does what it does on
    /// any other day.
    pub(super) fn on_mouse(&mut self, mouse: MouseEvent) {
        if !matches!(self.mode, Mode::Loading | Mode::List) {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                self.click_button(Position::new(mouse.column, mouse.row));
            }
            return;
        }
        let position = Position::new(mouse.column, mouse.row);
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if self.click_box(position) {
                return;
            }
            self.typing = Typing::Nothing;
        } else if self.typing != Typing::Nothing {
            return;
        }
        let Some(pane) = self.drawn.areas.at(position) else {
            return;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = pane;
                match pane {
                    Pane::Comments => self.click_comment(position),
                    Pane::Description => self.click_description(position),
                    Pane::Checks => self.click_check(position),
                    _ => {}
                }
                if let Some(sort) = self.sort_at(position) {
                    self.click_sort(sort);
                }
                if self.drawn.areas.table_rows.contains(position) {
                    let row = usize::from(position.y - self.drawn.areas.table_rows.y);
                    // Below the last row there is nothing to click.
                    let index = self.drawn.areas.table_offset + row;
                    if let Some(&row) = self.visible.get(index) {
                        // A click on the checkbox highlights the row too, so its diff can be
                        // read before moving on to the next.
                        self.highlight(index);
                        if self.drawn.areas.table_checkboxes.contains(position) {
                            self.toggle(self.rows[row].pr.number);
                        }
                    }
                }
            }
            // The wheel acts on the pane under the pointer and leaves the focus where it is.
            // Shift with the wheel scrolls sideways in terminals that don't send ScrollLeft.
            MouseEventKind::ScrollDown if mouse.modifiers.contains(KeyModifiers::SHIFT) => {
                self.scroll_sideways(pane, SIDEWAYS_COLUMNS)
            }
            MouseEventKind::ScrollUp if mouse.modifiers.contains(KeyModifiers::SHIFT) => {
                self.scroll_sideways(pane, -SIDEWAYS_COLUMNS)
            }
            MouseEventKind::ScrollRight => self.scroll_sideways(pane, SIDEWAYS_COLUMNS),
            MouseEventKind::ScrollLeft => self.scroll_sideways(pane, -SIDEWAYS_COLUMNS),
            MouseEventKind::ScrollDown => self.wheel(pane, 1),
            MouseEventKind::ScrollUp => self.wheel(pane, -1),
            _ => {}
        }
    }

    /// Presses the dialog's button under `position`, if there is one: what Enter does with that
    /// button highlighted.
    fn click_button(&mut self, position: Position) {
        let Some(index) = self
            .drawn
            .buttons
            .iter()
            .position(|button| button.contains(position))
        else {
            return;
        };
        match &mut self.mode {
            Mode::Confirm(confirm) => {
                confirm.choice = index;
                self.on_key(KeyCode::Enter.into());
            }
            Mode::ConfirmOpen(confirm) => {
                confirm.open = index == 0;
                self.on_key(KeyCode::Enter.into());
            }
            // Enter does not close these, but Esc and `q` do, and OK says the same.
            Mode::Help | Mode::Results(_) => self.mode = Mode::List,
            _ => {}
        }
    }

    /// Starts editing the box under `position`, if there is one, with the cursor at the clicked
    /// column. Returns whether there was one.
    fn click_box(&mut self, position: Position) -> bool {
        let (typing, area) = if self.drawn.areas.search.contains(position) {
            (Typing::Search, self.drawn.areas.search)
        } else if self.drawn.areas.filter.contains(position) {
            (Typing::Filter, self.drawn.areas.filter)
        } else if self.drawn.areas.columns.contains(position) {
            (Typing::Columns, self.drawn.areas.columns)
        } else {
            return false;
        };
        self.typing = typing;
        let input = match typing {
            Typing::Filter => &mut self.filter,
            Typing::Columns => &mut self.columns_input,
            _ => &mut self.query,
        };
        // The text starts inside the border, scrolled to keep the cursor in view.
        let width = usize::from(area.width.saturating_sub(2));
        let column =
            usize::from(position.x.saturating_sub(area.x + 1)) + input.visual_scroll(width);
        let index = char_at_column(input.value(), column);
        input.handle(InputRequest::SetCursor(index));
        true
    }

    pub(super) fn on_key(&mut self, key: KeyEvent) {
        self.note = None;
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if ctrl_c {
            self.should_quit = true;
            return;
        }
        match &mut self.mode {
            Mode::Confirm(confirm) => {
                if let Some(choice) = on_confirm_key(confirm, key) {
                    self.choose(choice);
                }
            }
            Mode::ConfirmOpen(confirm) => match confirm.on_key(key) {
                Some(true) => {
                    self.prs_to_open = std::mem::take(&mut confirm.numbers);
                    self.mode = Mode::List;
                }
                Some(false) => self.mode = Mode::List,
                None => {}
            },
            Mode::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?' | 'q')) {
                    self.mode = Mode::List;
                }
            }
            Mode::Running(batch) => {
                if key.code == KeyCode::Char('q') {
                    batch.cancel();
                    self.finish_batch();
                }
            }
            Mode::Results(_) => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
                    self.mode = Mode::List;
                }
            }
            Mode::Loading | Mode::List => match self.typing {
                Typing::Search => self.on_search_key(key),
                Typing::Filter => self.on_filter_key(key),
                Typing::Columns => self.on_columns_key(key),
                Typing::Nothing => self.on_list_key(key),
            },
        }
    }

    /// While editing, every key other than Enter and Esc goes to the search box.
    fn on_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                // With a suggestion chosen, Enter takes it and does not search.
                if !self.accept_completion() {
                    self.typing = Typing::Nothing;
                    self.submitted_query = self.query.value().to_string();
                    self.history.record(&self.submitted_query);
                    self.list.query_to_fetch = Some(self.submitted_query.clone());
                    self.list.refresh_requested = false;
                    self.errors.clear();
                }
            }
            // The list of suggestions, when it is open, comes before the box in what Esc and Up and
            // Down do.
            KeyCode::Esc => {
                if !self.dismiss_completion() {
                    self.typing = Typing::Nothing;
                    self.query = Input::new(self.submitted_query.clone());
                }
            }
            // Tab and Shift-Tab do what Down and Up do in the list, and nothing without one.
            KeyCode::Tab | KeyCode::BackTab => {
                self.move_completion(if key.code == KeyCode::Tab { 1 } else { -1 });
            }
            KeyCode::Up | KeyCode::Down => {
                let step = if key.code == KeyCode::Up { -1 } else { 1 };
                if !self.move_completion(step) {
                    let current = self.query.value();
                    let recalled = match key.code {
                        KeyCode::Up => self.history.previous(current),
                        _ => self.history.next(current),
                    };
                    if let Some(recalled) = recalled {
                        self.query = Input::new(recalled.to_string());
                        self.hide_completion();
                    }
                }
            }
            _ => {
                self.query.handle_event(&Event::Key(key));
            }
        }
    }

    /// While filtering, every key other than Enter and Esc goes to the filter box, and the list
    /// is filtered as you type. Enter keeps the filter; Esc clears it.
    fn on_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.typing = Typing::Nothing,
            KeyCode::Esc => self.clear_filter(),
            _ => {
                self.filter.handle_event(&Event::Key(key));
                self.refilter();
            }
        }
    }

    /// While editing the columns, every key other than Enter and Esc goes to the columns box.
    /// Enter applies the regex; Esc drops the edit.
    fn on_columns_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.apply_columns(),
            KeyCode::Esc => self.typing = Typing::Nothing,
            _ => {
                self.columns_input.handle_event(&Event::Key(key));
                self.columns_error = None;
            }
        }
    }

    fn on_list_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('o') => self.open_selected(),
                KeyCode::Char('g') => self.review_selected(),
                KeyCode::Char('a') => self.select_passing(),
                KeyCode::Char('r') => self.refresh_list(),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('/') => self.typing = Typing::Search,
            KeyCode::Char('f') => self.typing = Typing::Filter,
            KeyCode::Char('c') => self.edit_columns(),
            KeyCode::Char('s') => self.cycle_sort(),
            KeyCode::Char('S') => self.reverse_sort(),
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('o') => self.open_focused(),
            KeyCode::Char('y') => self.copy_focused(),
            KeyCode::Char('p') => self.toggle_refresh_paused(),
            KeyCode::Char('q') => self.should_quit = true,
            // Esc never quits: it is too easy to hit by accident. It clears the filter, then
            // the selection.
            KeyCode::Esc if !self.filter.value().is_empty() => self.clear_filter(),
            KeyCode::Esc => self.selected.clear(),
            KeyCode::Tab => self.focus = self.focus.next(),
            KeyCode::BackTab => self.focus = self.focus.previous(),
            KeyCode::Down | KeyCode::Char('j') => self.step_focused(1),
            KeyCode::Up | KeyCode::Char('k') => self.step_focused(-1),
            KeyCode::Right | KeyCode::Char('l') => {
                self.scroll_sideways(self.focus, SIDEWAYS_COLUMNS)
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.scroll_sideways(self.focus, -SIDEWAYS_COLUMNS)
            }
            KeyCode::PageDown => self.page_focused(1),
            KeyCode::PageUp => self.page_focused(-1),
            KeyCode::Home => self.jump_focused(false),
            KeyCode::End => self.jump_focused(true),
            KeyCode::Char(' ') if self.focus == Pane::Comments => self.toggle_comment(),
            KeyCode::Char(' ') => self.toggle_selected(),
            KeyCode::Enter => self.enter_focused(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::char_at_column;

    #[test]
    fn a_column_maps_to_the_character_drawn_there() {
        assert_eq!(char_at_column("is:pr", 0), 0);
        assert_eq!(char_at_column("is:pr", 3), 3);
        assert_eq!(char_at_column("is:pr", 5), 5);
        assert_eq!(char_at_column("is:pr", 40), 5);
        assert_eq!(char_at_column("", 3), 0);
    }

    #[test]
    fn a_wide_character_takes_two_columns() {
        // `日` is columns 1 and 2, so a click on either is on it.
        assert_eq!(char_at_column("a日b", 1), 1);
        assert_eq!(char_at_column("a日b", 2), 1);
        assert_eq!(char_at_column("a日b", 3), 2);
    }
}
