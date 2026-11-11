//! Editing the text of the search, filter and columns boxes, the way a shell's line editor does.
//!
//! `tui-input` has most of it: Ctrl-W, Alt-Backspace, Ctrl-Left/Right, Ctrl-A/E/K/U/Y, Home/End
//! and Ctrl-B/F/H. But it reads Alt as crossterm's `META`, which is the Super key, so Alt-B,
//! Alt-F and Alt-D do nothing in a terminal (which reports Alt as `ALT`), Alt-D even deletes the
//! word before the cursor, and Alt-Left/Right are not there at all. [`edit`] adds those, and is
//! the one place that all three boxes send their keys to.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tui_input::backend::crossterm::EventHandler;
use tui_input::{Input, InputRequest};

/// What the key asks for that `tui-input` doesn't understand: moving and deleting by word with
/// Alt (or Option, which terminals send as Alt, or as Alt-B and Alt-F for the arrows).
fn alt_request(key: KeyEvent) -> Option<InputRequest> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    // Alt or Super, but not Ctrl, which has its own meanings.
    let alt = key
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::META)
        && !key.modifiers.contains(KeyModifiers::CONTROL);
    if !alt {
        return None;
    }
    match key.code {
        KeyCode::Char('b') | KeyCode::Left => Some(InputRequest::GoToPrevWord),
        KeyCode::Char('f') | KeyCode::Right => Some(InputRequest::GoToNextWord),
        KeyCode::Char('d') | KeyCode::Delete => Some(InputRequest::DeleteNextWord),
        KeyCode::Backspace => Some(InputRequest::DeletePrevWord),
        _ => None,
    }
}

/// Applies `key` to `input`. Returns whether the text changed.
pub fn edit(input: &mut Input, key: KeyEvent) -> bool {
    let before = input.value().to_string();
    match alt_request(key) {
        Some(request) => {
            input.handle(request);
        }
        None => {
            input.handle_event(&Event::Key(key));
        }
    }
    input.value() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    /// A box holding `text` with the cursor after `cursor` characters.
    fn input(text: &str, cursor: usize) -> Input {
        Input::new(text.to_string()).with_cursor(cursor)
    }

    const ALT: KeyModifiers = KeyModifiers::ALT;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const NONE: KeyModifiers = KeyModifiers::NONE;

    #[test]
    fn alt_b_and_f_and_alt_arrows_move_by_word() {
        // What a terminal sends: Option-Left as Alt-B or as Alt with the arrow, depending on it.
        for back in [key(KeyCode::Char('b'), ALT), key(KeyCode::Left, ALT)] {
            let mut i = input("is:pr label:deps", 16);
            edit(&mut i, back);
            assert_eq!(i.cursor(), 6, "{back:?}");
            edit(&mut i, back);
            assert_eq!(i.cursor(), 0, "{back:?}");
        }
        for forward in [key(KeyCode::Char('f'), ALT), key(KeyCode::Right, ALT)] {
            let mut i = input("is:pr label:deps", 0);
            edit(&mut i, forward);
            assert_eq!(i.cursor(), 6, "{forward:?}");
            assert_eq!(i.value(), "is:pr label:deps");
        }
    }

    #[test]
    fn alt_d_and_alt_delete_delete_the_word_after_the_cursor() {
        for delete in [key(KeyCode::Char('d'), ALT), key(KeyCode::Delete, ALT)] {
            let mut i = input("one two three", 4);
            assert!(edit(&mut i, delete), "{delete:?}");
            assert_eq!(i.value(), "one three", "{delete:?}");
            assert_eq!(i.cursor(), 4);
        }
    }

    /// A canary for upgrading `tui-input`: these keys are its own, not ours, so a failure here
    /// means a new version changed them.
    #[test]
    fn tui_input_still_handles_the_word_and_shell_keys() {
        // Alt-Backspace and Ctrl-W delete the word before the cursor.
        for delete in [key(KeyCode::Backspace, ALT), key(KeyCode::Char('w'), CTRL)] {
            let mut i = input("one two three", 7);
            edit(&mut i, delete);
            assert_eq!(i.value(), "one  three", "{delete:?}");
        }
        let mut i = input("one two three", 13);
        edit(&mut i, key(KeyCode::Left, CTRL));
        assert_eq!(i.cursor(), 8);
        let mut i = input("one two", 3);
        edit(&mut i, key(KeyCode::Char('k'), CTRL));
        assert_eq!(i.value(), "one");
    }

    #[test]
    fn typing_still_inserts_and_an_unknown_alt_key_does_not() {
        let mut i = input("ab", 2);
        assert!(edit(&mut i, key(KeyCode::Char('c'), NONE)));
        assert!(edit(&mut i, key(KeyCode::Char('D'), KeyModifiers::SHIFT)));
        assert_eq!(i.value(), "abcD");
        assert!(!edit(&mut i, key(KeyCode::Char('x'), ALT)));
        assert_eq!(i.value(), "abcD");
    }
}
