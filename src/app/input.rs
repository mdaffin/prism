//! Keys and mouse clicks, routed by what the app is showing.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{App, Mode, Pane, Typing};
use crate::batch::on_confirm_key;

/// The columns that a key press or a notch of the wheel scrolls a pane sideways.
const SIDEWAYS_COLUMNS: isize = 8;

impl App {
    /// The mouse is ignored while a dialog is open or a box is being edited.
    pub(super) fn on_mouse(&mut self, mouse: MouseEvent) {
        if !matches!(self.mode, Mode::Loading | Mode::List) || self.typing != Typing::Nothing {
            return;
        }
        let position = Position::new(mouse.column, mouse.row);
        let Some(pane) = self.drawn.areas.at(position) else {
            return;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.focus = pane;
                match pane {
                    Pane::Comments => self.click_comment(position),
                    Pane::Checks => self.click_check(position),
                    _ => {}
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
                Typing::Nothing => self.on_list_key(key),
            },
        }
    }

    /// While editing, every key other than Enter and Esc goes to the search box.
    fn on_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.typing = Typing::Nothing;
                self.submitted_query = self.query.value().to_string();
                self.list.query_to_fetch = Some(self.submitted_query.clone());
                self.list.refresh_requested = false;
                self.errors.clear();
            }
            KeyCode::Esc => {
                self.typing = Typing::Nothing;
                self.query = Input::new(self.submitted_query.clone());
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
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('o') => self.open_focused(),
            KeyCode::Char('y') => self.copy_focused(),
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
