//! Selecting text in the description and the comments with the mouse, and copying it.
//!
//! The terminal's own selection is off while the mouse is captured, so a drag inside one of these
//! panes selects here. It works on the rows the view laid out (what a click on a link uses too),
//! not on screen cells, so a line that was wrapped is copied as the one line it was and the
//! border and the scrollbar are never in it. The text is copied when the button is released, or
//! with `y`, as OSC 52.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{App, Pane};
use crate::model::Join;

/// One row of a pane's laid-out text.
#[derive(Debug, Clone, Default)]
pub struct TextRow {
    /// What the row shows, as drawn.
    pub text: String,
    pub join: Join,
    /// Whether it is a row of a table, whose box characters are not copied.
    pub table: bool,
    /// Whether it is the header of a comment, which a click expands rather than selects.
    pub header: bool,
}

/// The text a pane drew, for selecting.
#[derive(Debug, Clone, Default)]
pub struct TextView {
    /// The cells the text is drawn in, inside the border.
    pub inner: Rect,
    /// The first row shown.
    pub scroll: usize,
    /// Shared, as a diff's rows are made once when it loads.
    pub rows: std::sync::Arc<Vec<TextRow>>,
    /// The columns the text is scrolled sideways, which the rows are not cut for (a diff's are).
    pub hscroll: usize,
}

/// A place in the text: a row and a column of it.
pub type Place = (usize, usize);

/// What is selected in one pane's text for one PR.
#[derive(Debug, Clone)]
pub struct Selection {
    pub pane: Pane,
    /// The PR whose text it is: for another, the selection means nothing.
    pub number: u64,
    anchor: Place,
    cursor: Place,
}

impl Selection {
    /// The ends in reading order.
    fn ordered(&self) -> (Place, Place) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// The columns of `row`, of `width`, that are selected, if any.
    pub fn columns_on(&self, row: usize, width: usize) -> Option<(usize, usize)> {
        let (start, end) = self.ordered();
        if row < start.0 || row > end.0 {
            return None;
        }
        let from = if row == start.0 { start.1 } else { 0 };
        // The cell the pointer is on is in it.
        let to = if row == end.0 { end.1 + 1 } else { width };
        (from < to).then_some((from, to.min(width.max(from + 1))))
    }

    /// The selected text. Rows of a wrapped line are joined as the line they were, a table's rows
    /// come out with a tab between the cells and no box, and the spaces that padded a row are left
    /// off.
    pub fn text(&self, view: &TextView) -> String {
        let (start, end) = self.ordered();
        let mut out = String::new();
        let mut first = true;
        for row_number in start.0..=end.0 {
            let Some(row) = view.rows.get(row_number) else {
                break;
            };
            let from = if row_number == start.0 { start.1 } else { 0 };
            let to = if row_number == end.0 {
                end.1 + 1
            } else {
                usize::MAX
            };
            let part = columns_of(&row.text, from, to);
            let part = if row.table {
                match table_cells(&part) {
                    Some(cells) => cells,
                    // A line of the table's frame has nothing in it.
                    None => continue,
                }
            } else {
                part.trim_end().to_string()
            };
            if !first {
                match row.join {
                    Join::New => out.push('\n'),
                    // The space between two words went to the end of the row before, or was
                    // dropped, and the row before has been trimmed.
                    Join::Word => {
                        if !out.ends_with(char::is_whitespace) {
                            out.push(' ');
                        }
                    }
                    Join::Split => {}
                }
            }
            out.push_str(&part);
            first = false;
        }
        out
    }
}

/// The part of `text` in the display columns `from` to `to`, not including `to`.
fn columns_of(text: &str, from: usize, to: usize) -> String {
    let mut column = 0;
    let mut part = String::new();
    for c in text.chars() {
        if column >= from && column < to {
            part.push(c);
        }
        column += c.width().unwrap_or(0);
    }
    part
}

/// The cells of a table row, apart from its box, with a tab between them. `None` for a row of
/// nothing but the frame.
fn table_cells(row: &str) -> Option<String> {
    let frame = |c: char| "┌┬┐├┼┤└┴┘─│ ".contains(c);
    if row.chars().all(frame) {
        return None;
    }
    let cells: Vec<&str> = row.split('│').map(str::trim).collect();
    // A row cut off by a partial selection can have no edge to the left or the right.
    let inner = match cells.as_slice() {
        [first, middle @ .., last] => {
            let mut all = Vec::new();
            if !first.is_empty() {
                all.push(*first);
            }
            all.extend(middle);
            if !last.is_empty() {
                all.push(*last);
            }
            all
        }
        other => other.to_vec(),
    };
    Some(inner.join("\t"))
}

impl App {
    fn text_view(&self, pane: Pane) -> Option<&TextView> {
        match pane {
            Pane::Description => Some(&self.drawn.description_text),
            Pane::Comments => Some(&self.drawn.comment_text),
            Pane::Diff => Some(&self.drawn.diff_text),
            _ => None,
        }
    }

    /// The selection, if it is of the highlighted PR's text and its pane is there.
    pub fn selection(&self) -> Option<&Selection> {
        let selection = self.selection.as_ref()?;
        let number = self.highlighted()?.pr.number;
        (selection.number == number && !self.pane_hidden(selection.pane)).then_some(selection)
    }

    /// The row and column of the text that `position` is over, for a pane's text. A position
    /// above or below the text is the first or last row shown, and one beside it is the first or
    /// last column. `None` if there is no text.
    fn place_at(view: &TextView, position: Position) -> Option<Place> {
        if view.rows.is_empty() || view.inner.is_empty() {
            return None;
        }
        let inner = view.inner;
        let x = position.x.clamp(inner.x, inner.right() - 1) - inner.x;
        let y = position.y.clamp(inner.y, inner.bottom() - 1) - inner.y;
        let row = (view.scroll + usize::from(y)).min(view.rows.len() - 1);
        Some((row, usize::from(x) + view.hscroll))
    }

    /// A press in the description or the comments: a drag from here selects text. It doesn't on a
    /// link or a comment's header, which a click acts on.
    pub(super) fn begin_selection(&mut self, pane: Pane, position: Position) {
        self.selection = None;
        self.select_from = None;
        let Some(number) = self.highlighted().map(|row| row.pr.number) else {
            return;
        };
        let Some(view) = self.text_view(pane) else {
            return;
        };
        if !view.inner.contains(position) {
            return;
        }
        let Some(place) = Self::place_at(view, position) else {
            return;
        };
        let links: &[Vec<crate::model::LinkSpan>] = match pane {
            Pane::Description => &self.drawn.description_links,
            Pane::Comments => &self.drawn.comment_links,
            _ => &[],
        };
        let on_link = links
            .get(place.0)
            .is_some_and(|row| row.iter().any(|l| l.start <= place.1 && place.1 < l.end));
        if view.rows[place.0].header || on_link {
            return;
        }
        self.select_from = Some(Selection {
            pane,
            number,
            anchor: place,
            cursor: place,
        });
    }

    /// A drag, or the button coming up, after a press that began a selection. Returns whether it
    /// was one, so that nothing else acts on it.
    pub(super) fn on_selection(&mut self, mouse: MouseEvent) -> bool {
        let Some(mut selection) = self.select_from.clone() else {
            return false;
        };
        let position = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(view) = self.text_view(selection.pane) else {
                    return false;
                };
                // Past the top or bottom of the text, the pane scrolls as the drag goes on.
                let by: isize = if position.y < view.inner.y {
                    -1
                } else if position.y >= view.inner.bottom() {
                    1
                } else {
                    0
                };
                if let Some(cursor) = Self::place_at(view, position) {
                    selection.cursor = cursor;
                }
                if by != 0 {
                    let scroll = match selection.pane {
                        Pane::Description => &mut self.scroll.description,
                        Pane::Diff => &mut self.scroll.diff,
                        _ => &mut self.scroll.comments,
                    };
                    *scroll = scroll.saturating_add_signed(by as i16);
                }
                if selection.cursor != selection.anchor {
                    self.selection = Some(selection.clone());
                }
                self.select_from = Some(selection);
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.select_from = None;
                if self.selection.is_some() {
                    self.copy_selection();
                }
                true
            }
            _ => false,
        }
    }

    /// Copies the selected text, if there is some, and says how much.
    pub(super) fn copy_selection(&mut self) -> bool {
        let Some(selection) = self.selection() else {
            return false;
        };
        let Some(view) = self.text_view(selection.pane) else {
            return false;
        };
        let text = selection.text(view);
        if text.is_empty() {
            return false;
        }
        let count = text.chars().count();
        self.text_to_copy = Some(text);
        let noun = if count == 1 {
            "character"
        } else {
            "characters"
        };
        self.note = Some(format!("Copied {count} {noun}"));
        true
    }

    /// The width in display columns of a row of text.
    pub fn row_width(row: &TextRow) -> usize {
        row.text.width()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(rows: &[(&str, Join, bool)]) -> TextView {
        TextView {
            inner: Rect::new(1, 1, 20, 10),
            scroll: 0,
            hscroll: 0,
            rows: std::sync::Arc::new(
                rows.iter()
                    .map(|&(text, join, table)| TextRow {
                        text: text.to_string(),
                        join,
                        table,
                        header: false,
                    })
                    .collect(),
            ),
        }
    }

    fn selection(anchor: Place, cursor: Place) -> Selection {
        Selection {
            pane: Pane::Description,
            number: 1,
            anchor,
            cursor,
        }
    }

    #[test]
    fn a_wrapped_line_is_copied_as_one_line_and_other_lines_stay_apart() {
        let v = view(&[
            ("one two ", Join::New, false),
            ("three", Join::Word, false),
            ("https://example.com/a", Join::New, false),
            ("/very/long", Join::Split, false),
            ("", Join::New, false),
            ("next", Join::New, false),
        ]);
        let all = selection((0, 0), (5, 3)).text(&v);
        assert_eq!(
            all,
            "one two three\nhttps://example.com/a/very/long\n\nnext"
        );
        // A word space that was dropped for room is put back.
        let v = view(&[("one two", Join::New, false), ("three", Join::Word, false)]);
        assert_eq!(selection((0, 0), (1, 4)).text(&v), "one two three");
    }

    #[test]
    fn a_selection_starts_and_ends_where_the_pointer_was_whichever_way_it_went() {
        let v = view(&[("abcdef", Join::New, false), ("ghijkl", Join::New, false)]);
        assert_eq!(selection((0, 2), (1, 1)).text(&v), "cdef\ngh");
        // Backwards gives the same text.
        assert_eq!(selection((1, 1), (0, 2)).text(&v), "cdef\ngh");
        assert_eq!(selection((0, 1), (0, 3)).text(&v), "bcd");
    }

    #[test]
    fn wide_characters_are_cut_by_the_columns_they_take() {
        let v = view(&[("a日b", Join::New, false)]);
        // `日` is columns 1 and 2.
        assert_eq!(selection((0, 1), (0, 1)).text(&v), "日");
        assert_eq!(selection((0, 0), (0, 3)).text(&v), "a日b");
    }

    #[test]
    fn a_table_is_copied_without_its_box_with_tabs_between_its_cells() {
        let v = view(&[
            ("before", Join::New, false),
            ("┌──────┬──────┐", Join::New, true),
            ("│ Name │ Val  │", Join::New, true),
            ("├──────┼──────┤", Join::New, true),
            ("│ a    │ 1    │", Join::New, true),
            ("└──────┴──────┘", Join::New, true),
            ("after", Join::New, false),
        ]);
        let all = selection((0, 0), (6, 4)).text(&v);
        assert_eq!(all, "before\nName\tVal\na\t1\nafter");
        // A drag that starts and ends in the middle of a cell has what is selected of it.
        let part = selection((2, 2), (4, 12)).text(&v);
        assert_eq!(part, "Name\tVal\na\t1");
    }

    #[test]
    fn the_columns_selected_on_a_row_are_those_between_the_ends() {
        let s = selection((1, 3), (3, 2));
        assert_eq!(s.columns_on(0, 10), None);
        assert_eq!(s.columns_on(1, 10), Some((3, 10)));
        assert_eq!(s.columns_on(2, 10), Some((0, 10)));
        assert_eq!(s.columns_on(3, 10), Some((0, 3)));
        assert_eq!(s.columns_on(4, 10), None);
    }
}
